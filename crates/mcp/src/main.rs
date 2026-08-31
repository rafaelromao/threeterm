//! `threeterm-mcp` — MCP adapter exposing the ThreeTerm domain command API
//! as agent tools.
//!
//! The production binary drives the `McpServer::run` newline-framed
//! JSON-RPC 2.0 loop over the process's stdin and stdout. No TTY is
//! required; the binary runs cleanly under any MCP-compatible client
//! (or under `cargo test --test mcp_bracket`).

use std::io::{self, BufReader};

fn main() -> io::Result<()> {
    let stdin = BufReader::new(io::stdin());
    let stdout = io::stdout();
    let mut reader = stdin;
    let mut writer = stdout.lock();
    let server = threeterm_mcp::server::McpServer::new();
    let _handled = server.run(&mut reader, &mut writer)?;
    Ok(())
}
