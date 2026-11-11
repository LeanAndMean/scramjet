mod agent_cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    scramjet::os::low_integrity();
    scramjet::os::keep_own_handles();
    match agent_cli::execute(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scramjet: {error}");
            ExitCode::FAILURE
        }
    }
}
