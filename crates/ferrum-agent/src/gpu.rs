//! `ferrum-cli gpu hold` (23.5-d, `docs/gpu-sharing.md` §3.6): an operator
//! wraps a process outside FERRUM (a model server) in a `Resident` lease of
//! a GPU group, so FERRUM's engine calls wait for room next to it.
//!
//! This is **not** an agent command: it starts a program, so it has no
//! schema and no MCP tool, and only the command line reaches it.

use std::ffi::OsString;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ferrum_engines::gpu::{describe_holders, Kind, Request};

use crate::config::AgentConfig;
use crate::envelope::{AgentError, ErrorCode};

/// What to hold and what to run.
#[derive(Debug, Clone)]
pub struct Hold {
    /// The GPU group (`network.gpu_groups`).
    pub group: String,
    /// The lease in MiB; `None` takes the whole group.
    pub lease_mib: Option<u64>,
    /// The holder's name in the lock record (default: the program's name).
    pub name: Option<String>,
    /// How long to wait for room.
    pub timeout: Duration,
    /// The program and its arguments.
    pub command: Vec<OsString>,
}

/// Takes the lease, runs the command, and releases the lease when the
/// command exits; returns the command's exit code. `note` receives what
/// happens (waiting, holding, released) for the operator.
///
/// On Unix the command runs in its own process group, and each Ctrl-C (or
/// SIGTERM) sent to this process is passed on to that group as SIGINT; the
/// lease is released only after the command has exited. The command
/// inherits the lease's lock file, so if this process is killed (SIGKILL,
/// out of memory) the lease stays held until the command exits too.
pub fn hold(config: &AgentConfig, h: &Hold, note: &mut dyn FnMut(&str)) -> Result<i32, AgentError> {
    let (program, args) =
        h.command.split_first().ok_or_else(|| AgentError::bad_request("give the command to run after --"))?;
    if !config.gpu.groups.iter().any(|(g, _)| *g == h.group) {
        return Err(AgentError::bad_request(format!("network.gpu_groups has no group {:?}", h.group))
            .hint("name a group of the operator configuration (FERRUM_AGENT_CONFIG)"));
    }
    let default_name = std::path::Path::new(program).file_name().unwrap_or(program.as_os_str());
    let name = h.name.clone().unwrap_or_else(|| default_name.to_string_lossy().into_owned());
    let req = Request {
        group: &h.group,
        holder: &name,
        kind: Kind::Resident,
        lease_mib: h.lease_mib,
        capacity_mib: config.gpu.capacity(&h.group),
    };
    let dir = config.gpu.lock_dir().map_err(|e| AgentError::internal(e.to_string()))?;
    let child_pid = Arc::new(AtomicU32::new(0));
    let interrupted = Arc::new(AtomicBool::new(false));
    forward_interrupts(child_pid.clone(), interrupted.clone());
    let mut said = false;
    let mut on_wait = |w: &ferrum_engines::gpu::Wait| {
        if !said {
            note(&format!("waiting for GPU group {}: {}", w.group, describe_holders(w.held)));
            said = true;
        }
        !interrupted.load(Ordering::SeqCst)
    };
    let lease = match dir.acquire(&req, h.timeout, &mut on_wait) {
        Ok(lease) => lease,
        Err(ferrum_engines::gpu::LockError::Cancelled) => return Ok(interrupted_before_start(note)),
        Err(e) => {
            return Err(AgentError::new(ErrorCode::EngineUnavailable, e.to_string())
                .hint("try again when the holders are done, or give a longer --timeout-s"))
        }
    };
    if interrupted.load(Ordering::SeqCst) {
        return Ok(interrupted_before_start(note));
    }
    // the command inherits the lease's lock: it stays held if this process is killed first
    lease.share_with_children().map_err(|e| AgentError::internal(format!("GPU lease: {e}")))?;
    let mut cmd = Command::new(program);
    cmd.args(args);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Ctrl-C at the terminal reaches only this process, which passes it on to the command's group
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| AgentError::bad_request(format!("{}: {e}", program.to_string_lossy())))?;
    child_pid.store(child.id(), Ordering::SeqCst);
    if interrupted.load(Ordering::SeqCst) {
        // a Ctrl-C between the check above and the store: pass it on now
        interrupt_group(child.id());
    }
    let lease_text = h.lease_mib.map_or_else(|| "the whole group".to_owned(), |m| format!("{m} MiB"));
    note(&format!("holding {lease_text} of GPU group {} for {name} (pid {})", h.group, child.id()));
    wait_exited(child.id());
    // cleared before the child is reaped, so a late Ctrl-C cannot reach a reused pid
    child_pid.store(0, Ordering::SeqCst);
    let status = child.wait();
    drop(lease);
    note(&format!("released GPU group {}", h.group));
    let status = status.map_err(|e| AgentError::internal(format!("waiting for {name}: {e}")))?;
    Ok(exit_code(status))
}

/// Blocks until the child has exited, without reaping it (its pid stays
/// taken until `wait`).
fn wait_exited(pid: u32) {
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) {
        use rustix::process::{waitid, WaitId, WaitIdOptions};
        while let Err(rustix::io::Errno::INTR) = waitid(WaitId::Pid(pid), WaitIdOptions::EXITED | WaitIdOptions::NOWAIT)
        {
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// SIGINT to the command's process group (its pid), so a wrapper script and
/// the server it started both get it.
fn interrupt_group(pid: u32) {
    #[cfg(unix)]
    if let Some(pid) = i32::try_from(pid).ok().and_then(rustix::process::Pid::from_raw) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::INT);
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Ctrl-C before the command started: nothing ran; the exit code of an
/// interrupted program.
fn interrupted_before_start(note: &mut dyn FnMut(&str)) -> i32 {
    note("interrupted; the command was not started");
    130
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return 128 + sig;
        }
    }
    status.code().unwrap_or(1)
}

/// Keeps Ctrl-C and SIGTERM from ending this process: before the command
/// starts they stop the wait; while it runs each is passed on to the
/// command's process group as SIGINT. (Windows: Ctrl-C reaches every
/// process of the console, the command included.)
fn forward_interrupts(child_pid: Arc<AtomicU32>, interrupted: Arc<AtomicBool>) {
    let result = ctrlc::set_handler(move || {
        interrupted.store(true, Ordering::SeqCst);
        match child_pid.load(Ordering::SeqCst) {
            0 => {}
            pid => interrupt_group(pid),
        }
    });
    if let Err(e) = result {
        log::warn!("gpu hold: Ctrl-C is not forwarded: {e}");
    }
}
