//! Serves the mock engine over `ferrum-engine/1`, to try FERRUM's AI tools
//! without a GPU or a model:
//!
//! ```text
//! cargo run -p ferrum-engines --example mock_server -- 127.0.0.1:8765
//! ```
#![allow(missing_docs)]

use std::sync::Arc;

use ferrum_engines::{EngineServer, MockEngine};

fn main() -> std::io::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:8765".into());
    let token = std::env::var("FERRUM_ENGINE_TOKEN").ok();
    let server = EngineServer::start(Arc::new(MockEngine::default()), &addr, token)?;
    println!("mock engine (ferrum-engine/1) listening on {}", server.url());
    loop {
        std::thread::park();
    }
}
