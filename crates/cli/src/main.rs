//! `threeterm` — Headless Automation CLI adapter for the ThreeTerm domain
//! command API.
//!
//! The production binary forwards argv to the dispatcher's pure dispatch
//! function and propagates the returned exit code.

use std::io;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();

    let mut stdout = io::stdout().lock();
    let mut stderr = io::stderr().lock();

    let exit = if args.len() == 2 && args[0] == "--lua-session" {
        let Some(config) = args[1].to_str() else {
            return ExitCode::from(2);
        };
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        threeterm_cli::dispatch::dispatch_lua_session(config, &mut stdin, &mut stdout, &mut stderr)
    } else {
        threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr)
    };

    match exit {
        0 => ExitCode::SUCCESS,
        code => ExitCode::from(code as u8),
    }
}
