use std::{env, process::ExitCode};

const USAGE: &str = "Usage: scramjet-rs smoke\n       scramjet-rs --help";

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);

    match (arguments.next(), arguments.next()) {
        (Some(command), None) if command == "smoke" => {
            println!("{}", scramjet_rs::smoke_message());
            ExitCode::SUCCESS
        }
        (None, None) => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        (Some(command), None) if command == "--help" || command == "-h" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}
