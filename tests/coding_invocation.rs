#[path = "support/coding_invocation.rs"]
mod support;
use serde_json::{Value, json};
use std::fs;
use support::{Dirs, command, server};
fn output(command: &mut std::process::Command) -> std::process::Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn two_fresh_executables_keep_complete_context_and_execute_model_selected_python() {
    for (names, poem, reader) in [
        (
            ["verse notes hé.md", "reader alternate.py"],
            "# Verse\r\nQuiet hé🙂\rFiles\0sing\n",
            "import sys\nwith open('verse notes hé.md','rb') as source:\n sys.stdout.buffer.write(source.read())\n",
        ),
        (
            ["another draft.md", "bytes printer.py"],
            "A different request\rline\n",
            "from pathlib import Path\nimport sys\nsys.stdout.buffer.write(Path('another draft.md').read_bytes())\n",
        ),
    ] {
        let dirs = Dirs::new();
        let prompts=[format!("Please create a short verse in {}. Finish this message once it is written.",names[0]),"Please write your own Python reader for the previous file and execute it to print all of its contents.".into()];
        let first = dirs.input("first.txt", prompts[0].as_bytes());
        let second = dirs.input("second.txt", prompts[1].as_bytes());
        let calls = vec![
            Some(("write", json!({"path":names[0],"content":poem}))),
            None,
            Some(("read", json!({"path":names[0]}))),
            Some(("write", json!({"path":names[1],"content":reader}))),
            Some(("python", json!({"path":names[1]}))),
            None,
        ];
        let (endpoint, server) = server(
            calls,
            prompts,
            2,
            poem.into(),
            dirs.project.clone(),
            names.map(str::to_owned),
        );
        let first = output(&mut command(&dirs, endpoint, &names, &first, None));
        let first_record: Value = serde_json::from_slice(&first.stdout).unwrap();
        assert_eq!(first_record["generation"], 1);
        assert_eq!(first_record["counters"]["creates"], 1);
        assert_eq!(first_record["counters"]["attempts"], 2);
        assert_eq!(first_record["disposition"], "complete");
        assert!(!dirs.project.join(names[1]).exists());
        let prior = dirs.input("first.json", &first.stdout);
        let second = output(&mut command(&dirs, endpoint, &names, &second, Some(&prior)));
        let second_record: Value = serde_json::from_slice(&second.stdout).unwrap();
        assert_eq!(second_record["header"], first_record["header"]);
        assert_eq!(second_record["generation"], 2);
        assert_eq!(
            second_record["counters"],
            json!({"creates":2,"reads":1,"processes":1,"attempts":6})
        );
        assert_eq!(second_record["worker_joined"], true);
        assert_eq!(second_record["transports_joined"], true);
        assert_eq!(
            second_record["export_producer_acknowledgement"],
            "unknown_on_recovery"
        );
        let prefix = first_record["conversation"].as_array().unwrap();
        assert_eq!(
            &second_record["conversation"].as_array().unwrap()[..prefix.len()],
            prefix
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(second_record["receipts"].as_array().unwrap().len(), 6);
        let ledger = fs::read_to_string(
            dirs.storage
                .join("ordinary-run/provider/provider-attempts.jsonl"),
        )
        .unwrap();
        assert_eq!(ledger.lines().count(), 13);
        for (index, receipt) in second_record["receipts"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
        {
            assert_eq!(receipt["ordinal"], index + 1);
            assert_eq!(receipt["durable"], true);
        }
        println!(
            "two independent processes: requests=6 generations=2 creates=2 reads=1 processes=1 byte_exact=true max_request={}",
            requests
                .iter()
                .map(|r| serde_json::to_vec(r).unwrap().len())
                .max()
                .unwrap()
        );
    }
}
#[test]
fn text_only_final_is_ordinary_per_message_completion() {
    let dirs = Dirs::new();
    let names = ["input.md", "chosen.py"];
    let prompt = dirs.input("prompt.txt", b"Tell me something short; do not use tools.");
    let (endpoint, server) = server(
        vec![None],
        [
            "Tell me something short; do not use tools.".into(),
            "".into(),
        ],
        usize::MAX,
        "".into(),
        dirs.project.clone(),
        names.map(str::to_owned),
    );
    let result = output(&mut command(&dirs, endpoint, &names, &prompt, None));
    let record: Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        record["counters"],
        json!({"creates":0,"reads":0,"processes":0,"attempts":1})
    );
    assert_eq!(record["disposition"], "complete");
    assert_eq!(server.join().unwrap().len(), 1);
}
