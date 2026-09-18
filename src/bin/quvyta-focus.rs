//! The `quvyta-focus` command, the long name of `qfocus`: a subcommand runs and exits, no subcommand opens the screens.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = qfocus::cli::run(&args, &mut std::io::stdout(), &mut std::io::stderr()) {
        return code;
    }
    match qfocus::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
