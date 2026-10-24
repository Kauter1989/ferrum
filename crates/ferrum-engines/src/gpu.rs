//! Engines that share a GPU (`docs/gpu-sharing.md`, ADR 0015).
//!
//! The operator puts engines that share a card into a *GPU group*
//! (`network.gpu_groups`). Every engine call of a group takes a lease of
//! `ferrum-gpu-lock/1` first, so FERRUM's agent, its desktop app and a model
//! server outside FERRUM (Medical Harness's `llama-server`) never use the
//! card beyond its size. [`GpuSettings`] holds the `[network]` settings; the
//! agent's and the desktop's configuration both parse them here.
//!
//! ```toml
//! [network]
//! gpu_groups = { card = ["http://127.0.0.1:8765", "http://127.0.0.1:8766"] }
//! gpu_capacity_mib = { card = 11264 }     # leases of the group must fit in it
//! gpu_peaks_mib = { "http://127.0.0.1:8765" = 6000 }   # whole-process GPU memory of a call
//! gpu_lock_dir = "/run/user/1000/ferrum/gpu"           # default: per user
//! ```

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrum_domain::{
    EngineError, EngineInfo, EngineObserver, InteractiveSession, JobStatus, Prompt, PromptResult, SegmentationEngine,
    Volume, VoxelBox,
};

pub use ferrum_gpu_lock::{default_path, describe_holders, HolderInfo, Kind, Lease, LockDir, LockError, Request, Wait};

/// The `[network]` GPU settings of the operator configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuSettings {
    /// Group name → engine URLs (without trailing `/`).
    pub groups: Vec<(String, Vec<String>)>,
    /// Group name → capacity in MiB; a group without one admits one holder
    /// at a time.
    pub capacity_mib: Vec<(String, u64)>,
    /// Engine URL → expected GPU memory of one call, in MiB (the whole
    /// engine process).
    pub peaks_mib: Vec<(String, u64)>,
    /// Lock directory; `None` uses the user's default.
    pub lock_dir: Option<PathBuf>,
}

/// The expected peak of a call: the engine's manifest may raise the
/// operator's value but never lower it (ADR 0015 §3). `None`: the call
/// takes the whole group.
pub fn effective_peak(operator: Option<u64>, manifest: Option<u64>) -> Option<u64> {
    operator.max(manifest)
}

impl GpuSettings {
    /// The group of an engine URL, if any.
    pub fn group(&self, url: &str) -> Option<&str> {
        self.groups.iter().find(|(_, l)| l.iter().any(|u| u == url)).map(|(g, _)| g.as_str())
    }

    /// Engine URLs of `group`.
    pub fn members(&self, group: &str) -> Vec<String> {
        self.groups.iter().filter(|(g, _)| g == group).flat_map(|(_, l)| l.clone()).collect()
    }

    /// Capacity of `group` in MiB.
    pub fn capacity(&self, group: &str) -> Option<u64> {
        self.capacity_mib.iter().find(|(g, _)| g == group).map(|(_, c)| *c)
    }

    /// The operator's expected peak of an engine.
    pub fn operator_peak(&self, url: &str) -> Option<u64> {
        self.peaks_mib.iter().find(|(u, _)| u == url).map(|(_, p)| *p)
    }

    /// Applies one `[network]` key. `Ok(false)`: not a GPU setting.
    pub fn set(&mut self, key: &str, v: &toml::Value) -> Result<bool, String> {
        let what = format!("network.{key}");
        match key {
            "gpu_groups" => {
                let t = v.as_table().ok_or_else(|| format!("{what} must be a table of URL lists"))?;
                self.groups = t
                    .iter()
                    .map(|(g, l)| Ok::<_, String>((g.clone(), urls(l, &format!("{what}.{g}"))?)))
                    .collect::<Result<_, _>>()?;
            }
            "gpu_capacity_mib" => self.capacity_mib = mib_table(v, &what, str::to_owned)?,
            "gpu_peaks_mib" => self.peaks_mib = mib_table(v, &what, |u| u.trim_end_matches('/').to_owned())?,
            "gpu_lock_dir" => {
                let p = PathBuf::from(v.as_str().ok_or_else(|| format!("{what} must be a path"))?);
                if !p.is_absolute() {
                    return Err(format!("{what} must be an absolute path"));
                }
                self.lock_dir = Some(p);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Checks references between the keys (they may come in any order): a
    /// capacity names a group, a peak names an engine of a group.
    pub fn validate(&self) -> Result<(), String> {
        let all: Vec<&String> = self.groups.iter().flat_map(|(_, l)| l).collect();
        if let Some(u) = all.iter().enumerate().find(|(n, u)| all[..*n].contains(u)).map(|(_, u)| u) {
            return Err(format!("network.gpu_groups lists {u} in more than one group"));
        }
        if let Some((g, _)) = self.capacity_mib.iter().find(|(g, _)| !self.groups.iter().any(|(n, _)| n == g)) {
            return Err(format!("network.gpu_capacity_mib names {g}, which is not in network.gpu_groups"));
        }
        if let Some((u, _)) = self.peaks_mib.iter().find(|(u, _)| self.group(u).is_none()) {
            return Err(format!("network.gpu_peaks_mib names {u}, which is in no GPU group"));
        }
        Ok(())
    }

    /// The lock directory (created and checked).
    pub fn lock_dir(&self) -> Result<LockDir, LockError> {
        match &self.lock_dir {
            Some(p) => LockDir::at(p),
            None => LockDir::default_for_user(),
        }
    }

    /// Takes the lease an engine call of `url` needs and holds it until the
    /// returned lease is dropped; `Ok(None)` when the engine is in no group.
    /// `manifest_peak` is the engine's `limits.expected_peak_mib`.
    /// `on_wait` is called between attempts; it returns `false` to stop.
    pub fn acquire(
        &self,
        url: &str,
        holder: &str,
        manifest_peak: Option<u64>,
        timeout: Duration,
        on_wait: &mut dyn FnMut(&Wait) -> bool,
    ) -> Result<Option<Lease>, LockError> {
        let Some(group) = self.group(url) else { return Ok(None) };
        let req = ferrum_gpu_lock::Request {
            group,
            holder,
            kind: Kind::Engine,
            lease_mib: effective_peak(self.operator_peak(url), manifest_peak),
            capacity_mib: self.capacity(group),
        };
        self.lock_dir()?.acquire(&req, timeout, on_wait).map(Some)
    }
}

impl GpuSettings {
    /// The `[network]` GPU settings of an operator configuration file, for
    /// the desktop app (the agent's `AgentConfig` reads the same keys); other
    /// sections and keys are left to their owners.
    pub fn load_network(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.message().to_owned())?;
        let mut s = Self::default();
        if let Some(network) = table.get("network").and_then(toml::Value::as_table) {
            for (k, v) in network {
                s.set(k, v)?;
            }
        }
        s.validate()?;
        Ok(s)
    }
}

/// An engine of a GPU group, as the desktop app uses it: opening a session
/// takes the group's lease and the session holds it until it is dropped,
/// because an open interactive session keeps its image on the GPU. While
/// the lease waits, the [`EngineObserver`] hears who holds the group and may
/// stop the wait.
pub struct GatedEngine {
    inner: Arc<dyn SegmentationEngine>,
    settings: GpuSettings,
    url: String,
    timeout: Duration,
    observer: Mutex<Option<Arc<dyn EngineObserver>>>,
}

impl GatedEngine {
    /// `inner` reached at `url`, gated by `settings` (no-op when the URL is in
    /// no group); a wait longer than `timeout` fails.
    pub fn new(inner: Arc<dyn SegmentationEngine>, settings: GpuSettings, url: &str, timeout: Duration) -> Self {
        let url = url.trim_end_matches('/').to_owned();
        Self { inner, settings, url, timeout, observer: Mutex::new(None) }
    }

    fn lease(&self) -> Result<Option<Lease>, EngineError> {
        let info = self.inner.info()?;
        let holder = format!("ferrum desktop: {}", info.name);
        let observer = self.observer.lock().ok().and_then(|o| o.clone());
        let mut on_wait = |w: &Wait| {
            let note = format!("GPU group {}: {}", w.group, describe_holders(w.held));
            observer.as_ref().is_none_or(|o| o.waiting(&note))
        };
        self.settings.acquire(&self.url, &holder, info.expected_peak_mib, self.timeout, &mut on_wait).map_err(|e| {
            match e {
                LockError::Cancelled => EngineError::Cancelled("stopped waiting for the GPU".into()),
                other => EngineError::Internal(other.to_string()),
            }
        })
    }
}

impl SegmentationEngine for GatedEngine {
    fn info(&self) -> Result<EngineInfo, EngineError> {
        self.inner.info()
    }

    fn open_session(&self, volume: &Volume, modality: &str) -> Result<Box<dyn InteractiveSession>, EngineError> {
        let lease = self.lease()?;
        let session = self.inner.open_session(volume, modality)?;
        Ok(Box::new(GatedSession { session, _lease: lease }))
    }

    fn unload(&self) -> Result<(), EngineError> {
        self.inner.unload()
    }

    fn observe(&self, observer: Arc<dyn EngineObserver>) {
        if let Ok(mut o) = self.observer.lock() {
            *o = Some(observer);
        }
    }
}

/// A session holding its group's lease. Fields drop in order: the session
/// closes (freeing the engine's memory) before the lease is released.
struct GatedSession {
    session: Box<dyn InteractiveSession>,
    _lease: Option<Lease>,
}

impl InteractiveSession for GatedSession {
    fn prompt(&mut self, prompt: &Prompt) -> Result<PromptResult, EngineError> {
        self.session.prompt(prompt)
    }
    fn mask(&mut self, bx: VoxelBox) -> Result<Vec<u8>, EngineError> {
        self.session.mask(bx)
    }
    fn undo(&mut self) -> Result<PromptResult, EngineError> {
        self.session.undo()
    }
    fn reset(&mut self) -> Result<(), EngineError> {
        self.session.reset()
    }
    fn start_job(&mut self, labels: Option<&[String]>) -> Result<String, EngineError> {
        self.session.start_job(labels)
    }
    fn job_status(&mut self, job: &str) -> Result<JobStatus, EngineError> {
        self.session.job_status(job)
    }
    fn cancel_job(&mut self, job: &str) -> Result<(), EngineError> {
        self.session.cancel_job(job)
    }
    fn label_map(&mut self) -> Result<Vec<u16>, EngineError> {
        self.session.label_map()
    }
}

fn urls(v: &toml::Value, what: &str) -> Result<Vec<String>, String> {
    let list = v.as_array().ok_or_else(|| format!("{what} must be a list of URLs"))?;
    list.iter()
        .map(|u| {
            u.as_str()
                .map(|s| s.trim_end_matches('/').to_owned())
                .ok_or_else(|| format!("{what} must be a list of URLs"))
        })
        .collect()
}

fn mib_table(v: &toml::Value, what: &str, key: impl Fn(&str) -> String) -> Result<Vec<(String, u64)>, String> {
    let t = v.as_table().ok_or_else(|| format!("{what} must be a table of MiB values"))?;
    t.iter()
        .map(|(k, n)| {
            let mib = n
                .as_integer()
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{what}.{k} must be a positive integer (MiB)"))?;
            Ok((key(k), mib.unsigned_abs()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<GpuSettings, String> {
        let table: toml::Table = text.parse().map_err(|e: toml::de::Error| e.to_string())?;
        let mut s = GpuSettings::default();
        for (k, v) in table["network"].as_table().unwrap() {
            assert!(s.set(k, v)?, "{k} is a GPU setting");
        }
        s.validate()?;
        Ok(s)
    }

    // covers 23.2-b
    #[test]
    fn peak_from_manifest_raises_never_lowers() {
        assert_eq!(effective_peak(Some(6000), Some(4000)), Some(6000), "an engine cannot lower the operator's peak");
        assert_eq!(effective_peak(Some(6000), Some(9000)), Some(9000), "it can raise it");
        assert_eq!(effective_peak(None, Some(4000)), Some(4000));
        assert_eq!(effective_peak(Some(6000), None), Some(6000));
        assert_eq!(effective_peak(None, None), None, "no peak: the call takes the whole group");
    }

    // covers 23.2-b, 23.2-d
    #[test]
    fn settings_are_parsed_and_cross_checked() {
        let s = parse(
            r#"[network]
            gpu_groups = { card = ["http://127.0.0.1:8765/", "http://127.0.0.1:8766"] }
            gpu_capacity_mib = { card = 11264 }
            gpu_peaks_mib = { "http://127.0.0.1:8765/" = 6000 }
            gpu_lock_dir = "/run/user/1000/ferrum/gpu""#,
        )
        .unwrap();
        assert_eq!(s.group("http://127.0.0.1:8765"), Some("card"), "trailing slashes are trimmed");
        assert_eq!(s.capacity("card"), Some(11264));
        assert_eq!(s.operator_peak("http://127.0.0.1:8765"), Some(6000));
        assert_eq!(s.operator_peak("http://127.0.0.1:8766"), None);
        assert_eq!(s.members("card").len(), 2);
        assert_eq!(s.lock_dir, Some(PathBuf::from("/run/user/1000/ferrum/gpu")));
        for (bad, says) in [
            ("[network]\ngpu_capacity_mib = { other = 1 }", "not in network.gpu_groups"),
            ("[network]\ngpu_peaks_mib = { \"http://x\" = 1 }", "in no GPU group"),
            ("[network]\ngpu_capacity_mib = { card = 0 }", "positive integer"),
            ("[network]\ngpu_capacity_mib = 5", "table of MiB"),
            ("[network]\ngpu_lock_dir = \"relative\"", "absolute path"),
            ("[network]\ngpu_groups = [1]", "table of URL lists"),
            ("[network]\ngpu_groups = { a = [1] }", "list of URLs"),
            ("[network]\ngpu_groups = { a = [\"http://x\"], b = [\"http://x/\"] }", "more than one group"),
        ] {
            let e = parse(bad).unwrap_err();
            assert!(e.contains(says), "{bad}: {e}");
        }
        let mut s = GpuSettings::default();
        assert_eq!(s.set("engines", &toml::Value::Integer(1)), Ok(false), "other keys are not ours");
    }

    // covers 23.2-a, 23.2-c
    #[test]
    fn engine_calls_of_a_group_take_their_peak_as_lease() {
        let dir = std::env::temp_dir().join(format!("ferrum-engines-gpu-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let url = "http://127.0.0.1:8765".to_owned();
        let s = GpuSettings {
            groups: vec![("card".into(), vec![url.clone(), "http://127.0.0.1:8766".into()])],
            capacity_mib: vec![("card".into(), 12_000)],
            peaks_mib: vec![(url.clone(), 7000)],
            lock_dir: Some(dir.clone()),
        };
        assert!(s.acquire("http://127.0.0.1:9", "t", None, Duration::ZERO, &mut |_| true).unwrap().is_none());
        let model = s
            .lock_dir()
            .unwrap()
            .acquire(
                &ferrum_gpu_lock::Request {
                    group: "card",
                    holder: "llama-server",
                    kind: Kind::Resident,
                    lease_mib: Some(6000),
                    capacity_mib: Some(12_000),
                },
                Duration::ZERO,
                &mut |_| true,
            )
            .unwrap();
        let full = s.acquire(&url, "ferrum", None, Duration::ZERO, &mut |_| true).unwrap_err();
        assert!(full.to_string().contains("llama-server"), "{full}");
        let lease = s.acquire(&url, "ferrum", Some(4000), Duration::ZERO, &mut |_| true);
        assert!(lease.is_err(), "the manifest cannot lower the operator's 7000 MiB");
        let other = s.acquire("http://127.0.0.1:8766", "ferrum", Some(4000), Duration::ZERO, &mut |_| true).unwrap();
        assert_eq!(other.unwrap().info().lease_mib, Some(4000));
        drop(model);
        let lease = s.acquire(&url, "ferrum", None, Duration::ZERO, &mut |_| true).unwrap().unwrap();
        assert_eq!(lease.info().lease_mib, Some(7000));
        drop(lease);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
