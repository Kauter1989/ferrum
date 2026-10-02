//! `ferrum-cli`: the FERRUM agent skill on the command line, and as an
//! MCP server (`ferrum-cli mcp`).
//!
//! Every command prints one `ferrum-agent/1` JSON envelope on stdout.
//! Exit codes: 0 success, 1 command error (see `error.code`), 2 usage
//! error. Points are written `v:i,j,k` (voxel), `mm:x,y,z` (LPS patient
//! millimetres) or `r-0001:x,y` (pixel of an earlier render).

use std::process::ExitCode;

use clap::Parser;
use ferrum_agent::mcp::McpServer;
use ferrum_agent::schema::command_schema;
use ferrum_agent::{Agent, AgentConfig};
use serde_json::{json, Map, Value};

mod cli;

use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut config = match cli.config.clone().or_else(|| std::env::var_os("FERRUM_AGENT_CONFIG").map(Into::into)) {
        Some(path) => match AgentConfig::load(&path) {
            Ok(c) => c,
            Err(e) => {
                println!("{}", ferrum_agent::envelope::error_envelope(&e));
                return ExitCode::from(1);
            }
        },
        None => AgentConfig::default(),
    };
    let (command, params) = match cli.command.to_call() {
        Ok(call) => call,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(2);
        }
    };
    match command.as_str() {
        "commands" => {
            println!("{}", json!({ "commands": Agent::commands().collect::<Vec<_>>() }));
            return ExitCode::SUCCESS;
        }
        "schema" => return print_schema(params["command"].as_str()),
        "mcp" => {
            if let Some(root) = params["workspace_root"].as_str() {
                match &config.workspace_root {
                    None => config.workspace_root = Some(root.into()),
                    Some(r) => {
                        eprintln!("ferrum-cli mcp: the operator configuration sets the workspace root {}", r.display())
                    }
                }
            }
            let mut server = McpServer::new(Agent::new(config));
            let stdin = std::io::stdin();
            return match server.serve(stdin.lock(), std::io::stdout().lock()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("ferrum-cli mcp: {e}");
                    ExitCode::from(1)
                }
            };
        }
        _ => {}
    }
    let envelope = Agent::new(config).run(&command, &params);
    println!("{}", serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string()));
    if envelope["ok"].as_bool() == Some(true) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Prints one schema, or all as `{command: {description, parameters}}`.
fn print_schema(command: Option<&str>) -> ExitCode {
    let entry = |c: &str| command_schema(c).map(|(d, s)| json!({ "description": d, "parameters": s }));
    let out = match command {
        Some(c) => match entry(c) {
            Some(v) => v,
            None => {
                eprintln!("error: unknown command {c:?}; see ferrum-cli commands");
                return ExitCode::from(2);
            }
        },
        None => Value::Object(Agent::commands().filter_map(|c| Some((c.to_owned(), entry(c)?))).collect()),
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_else(|_| out.to_string()));
    ExitCode::SUCCESS
}

/// Builds a JSON object from optional fields, leaving out `None`.
pub(crate) fn object(fields: Vec<(&str, Option<Value>)>) -> Value {
    let map: Map<String, Value> = fields.into_iter().filter_map(|(k, v)| v.map(|v| (k.to_owned(), v))).collect();
    Value::Object(map)
}
