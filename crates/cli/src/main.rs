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

    let exit = threeterm_cli::dispatch::dispatch(args, &mut stdout, &mut stderr);

    match exit {
        0 => ExitCode::SUCCESS,
        code => ExitCode::from(code as u8),
    }
}
