mod adaptive_demo;
#[path = "../tests/support/adaptive_fixture.rs"]
mod adaptive_fixture;
mod coding_cli;
mod coding_demo;
mod coding_ui_demo;
#[path = "../tests/support/coding_ui_fixture.rs"]
mod coding_ui_fixture;
mod command_demo;
#[path = "../tests/support/commands.rs"]
mod command_fixture;
mod command_reader_ui_demo;
mod command_readers_demo;
mod command_ui_demo;
mod cooldown_demo;
#[path = "../tests/support/cooldown_fixture.rs"]
mod cooldown_fixture;
mod demo;
mod haiku_demo;
mod haiku_ui_demo;
mod pacing_demo;
mod parent_demo;
mod project_demo;
#[allow(dead_code)]
#[path = "../tests/support/project_fixture.rs"]
mod project_fixture;
mod project_ui_demo;
mod provider_demo;
mod reader_ui_demo;
#[path = "../tests/support/reader_ui_fixture.rs"]
mod reader_ui_fixture;
mod runtime_ui_demo;
#[path = "../tests/support/runtime_ui_fixture.rs"]
mod runtime_ui_fixture;
mod token_demo;
mod ui;
use scramjet_rs::{ProjectRequest, ProjectSnapshot, RequestId, Runtime};
use std::{env, process::ExitCode};
#[cfg(test)]
static RUNTIME_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[test]
fn provider_report_writer_preserves_its_byte_budget() {
    provider_demo::check_report_writer_budget();
}

const USAGE: &str = "Usage: scramjet-rs smoke\n       scramjet-rs stream-demo\n       scramjet-rs coding-demo\n       scramjet-rs coding --owned-fixture --endpoint ADDR --project PATH --storage PATH --run ID --file NAME --file NAME --prompt-file PATH [--record-file PATH] [--approve-effects]\n       scramjet-rs parent-demo\n       scramjet-rs cooldown-demo\n       scramjet-rs pacing-demo\n       scramjet-rs token-demo\n       scramjet-rs adaptive-demo\n       scramjet-rs command-demo [--automatic | --resume]\n       scramjet-rs command-coding-demo\n       scramjet-rs command-readers-demo\n       scramjet-rs provider-demo --live|--fixture --storage PATH [--tool] [--retain-unknown-through N]\n       scramjet-rs haiku-demo --fixture --project PATH --storage PATH [--approve-effects] [--session ID]\n       scramjet-rs haiku-demo --history --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --fixture --project PATH --storage PATH [--session ID]\n       scramjet-rs haiku-ui-demo --history --storage PATH [--session ID]\n       scramjet-rs project-demo --project PATH --storage PATH --request JSON [--approve-edit] [--verify-edited] [--second-request JSON] [--session ID]\n       scramjet-rs project-demo --history --storage PATH [--session ID]\n       scramjet-rs project-ui-demo --project PATH --storage PATH [--session ID] [--fixture golden|reader-failure|model-failure|unknown-usage]\n       scramjet-rs command-ui-demo [--automatic]\n       scramjet-rs ui-demo [--diagnostic]\n       scramjet-rs runtime-ui-demo\n       scramjet-rs reader-ui-demo [--command]\n       scramjet-rs coding-ui-demo [--follow-up | --command]\n       scramjet-rs --help";

fn main() -> ExitCode {
    let mut arguments = env::args_os().skip(1);

    let command = arguments.next();
    if command.as_ref().is_some_and(|command| command == "coding") {
        return match coding_cli::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                scramjet_rs::CodingInvocationResult::report_failure(&error);
                ExitCode::FAILURE
            }
        };
    }
    if command
        .as_ref()
        .is_some_and(|command| command == "provider-demo")
    {
        return match provider_demo::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("provider-demo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if command
        .as_ref()
        .is_some_and(|command| command == "haiku-ui-demo")
    {
        return match haiku_ui_demo::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("haiku-ui-demo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if command
        .as_ref()
        .is_some_and(|command| command == "haiku-demo")
    {
        return match haiku_demo::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("haiku-demo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if command
        .as_ref()
        .is_some_and(|command| command == "project-demo")
    {
        return match project_demo::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("project-demo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if command
        .as_ref()
        .is_some_and(|command| command == "project-ui-demo")
    {
        return match project_ui_demo::execute(arguments) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("project-ui-demo: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let option = arguments.next();
    let extra = arguments.next();
    if extra.is_some() {
        eprintln!("{USAGE}");
        return ExitCode::FAILURE;
    }

    match (command, option) {
        (Some(command), None) if command == "smoke" => {
            println!("{}", scramjet_rs::smoke_message());
            ExitCode::SUCCESS
        }
        (Some(command), None) if command == "stream-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("stream-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "coding-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(coding_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("coding-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "cooldown-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(cooldown_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("cooldown-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "pacing-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(pacing_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("pacing-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "token-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(token_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("token-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "adaptive-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(adaptive_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("adaptive-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "parent-demo" => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match runtime.block_on(parent_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("parent-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), option)
            if command == "command-demo"
                && option
                    .as_ref()
                    .is_none_or(|flag| flag == "--automatic" || flag == "--resume") =>
        {
            match command_demo::run(
                option.as_ref().is_some_and(|f| f == "--automatic"),
                option.as_ref().is_some_and(|f| f == "--resume"),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("command-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "command-readers-demo" => {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match executor.block_on(command_readers_demo::run()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("command-readers-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "command-coding-demo" => {
            let executor = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("local executor");
            match executor.block_on(coding_ui_demo::command_demo()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("command-coding-demo: {error}");
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), option)
            if command == "command-ui-demo"
                && option.as_ref().is_none_or(|flag| flag == "--automatic") =>
        {
            match command_ui_demo::run(option.is_some()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    command_ui_demo::report_error(&error);
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), option)
            if command == "coding-ui-demo"
                && option
                    .as_ref()
                    .is_none_or(|flag| flag == "--follow-up" || flag == "--command") =>
        {
            match coding_ui_demo::run(
                option.as_ref().is_some_and(|f| f == "--follow-up"),
                option.as_ref().is_some_and(|f| f == "--command"),
            ) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    coding_ui_demo::report_error(&error);
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), option)
            if command == "reader-ui-demo" && option.as_ref().is_none_or(|f| f == "--command") =>
        {
            match if option.is_some() {
                command_reader_ui_demo::run()
            } else {
                reader_ui_demo::run()
            } {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    reader_ui_demo::report_error(&error);
                    ExitCode::FAILURE
                }
            }
        }
        (Some(command), None) if command == "runtime-ui-demo" => match runtime_ui_demo::run() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                runtime_ui_demo::report_error(&error);
                ExitCode::FAILURE
            }
        },
        (Some(command), option)
            if command == "ui-demo"
                && option.as_ref().is_none_or(|flag| flag == "--diagnostic") =>
        {
            match ui::run(option.is_some()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("ui-demo: {error}");
                    ExitCode::FAILURE
                }
            }
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
