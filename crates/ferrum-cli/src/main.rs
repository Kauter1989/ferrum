//! `ferrum-cli`: the FERRUM agent skill on the command line.
//!
//! Every command prints one `ferrum-agent/1` JSON envelope on stdout.
//! Exit codes: 0 success, 1 command error (see `error.code`), 2 usage
//! error. Points are written `v:i,j,k` (voxel), `mm:x,y,z` (LPS patient
//! millimetres) or `r-0001:x,y` (pixel of an earlier render).

use std::process::ExitCode;

use clap::Parser;
use ferrum_agent::{Agent, AgentConfig};
use serde_json::{json, Map, Value};

mod cli;

use cli::Cli;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let config = match cli.config.clone().or_else(|| std::env::var_os("FERRUM_AGENT_CONFIG").map(Into::into)) {
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
    if command == "commands" {
        println!("{}", json!({ "commands": Agent::commands().collect::<Vec<_>>() }));
        return ExitCode::SUCCESS;
    }
    let envelope = Agent::new(config).run(&command, &params);
    println!("{}", serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string()));
    if envelope["ok"].as_bool() == Some(true) {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Builds a JSON object from optional fields, leaving out `None`.
pub(crate) fn object(fields: Vec<(&str, Option<Value>)>) -> Value {
    let map: Map<String, Value> = fields.into_iter().filter_map(|(k, v)| v.map(|v| (k.to_owned(), v))).collect();
    Value::Object(map)
}
