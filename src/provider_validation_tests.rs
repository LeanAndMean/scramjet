use super::*;
use std::os::unix::fs::DirBuilderExt;
fn storage() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "validation-unit-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .unwrap();
    path
}
#[test]
fn pair_checks_in_isolated_processes() {
    for case in [
        "success",
        "success-max",
        "no-reasoning",
        "nonfit-max",
        "evidence-budget",
        "extra-text",
        "extra-message",
        "over-usage",
        "missing-usage",
        "partial-usage",
        "bad-call",
        "partial-body",
        "cancel-before-eof",
        "deadline-before-eof",
        "unsupported",
        "non-fit",
        "trailing",
        "metadata-change",
        "cancel",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_validation::tests::pair_child",
                "--nocapture",
            ])
            .env("VALIDATION_UNIT_CASE", case)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().filter(|line| {
            line.starts_with("pair-requested-heap:") || line.starts_with("pair-request-bytes:")
        }) {
            println!("{line}");
        }
        let root = stdout
            .lines()
            .find_map(|line| line.strip_prefix("PAIR_RECOVERY|"))
            .unwrap();
        let file = Path::new(root).join("ledger/provider-attempts.jsonl");
        let before = std::fs::read(&file).unwrap();
        let recovery = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider_validation::tests::pair_recovery_child",
                "--nocapture",
            ])
            .env("VALIDATION_RECOVERY_ROOT", root)
            .env("VALIDATION_RECOVERY_CASE", case)
            .output()
            .unwrap();
        assert!(
            recovery.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&recovery.stdout),
            String::from_utf8_lossy(&recovery.stderr)
        );
        assert_eq!(std::fs::read(file).unwrap(), before);
    }
}
#[test]
fn pair_recovery_child() {
    let Ok(root) = std::env::var("VALIDATION_RECOVERY_ROOT") else {
        return;
    };
    let case = std::env::var("VALIDATION_RECOVERY_CASE").unwrap();
    let root = Path::new(&root);
    let checkpoint: ValidationCheckpoint =
        serde_json::from_slice(&std::fs::read(root.join("evidence/checkpoint.json")).unwrap())
            .unwrap();
    let path = root.join("ledger");
    let before = std::fs::read(path.join("provider-attempts.jsonl")).unwrap();
    let history = provider_history::observe_validation(&path, &checkpoint).unwrap();
    assert_eq!(
        history.consumed(),
        if case.starts_with("success") || case == "no-reasoning" || case == "bad-call" {
            6
        } else if case == "cancel" {
            4
        } else {
            5
        }
    );
    if case != "cancel" {
        assert!(
            ProviderLedger::validation_open(&path, checkpoint.binding.endpoint, checkpoint.sha256)
                .is_err()
        );
        assert!(crate::reconstruct_provider_history(&path, checkpoint.binding.endpoint).is_err());
    }
    assert_eq!(
        std::fs::read(path.join("provider-attempts.jsonl")).unwrap(),
        before
    );
}
#[test]
fn pair_child() {
    let Ok(case) = std::env::var("VALIDATION_UNIT_CASE") else {
        return;
    };
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
        let root=storage();let ledger_path=root.join("ledger");std::fs::DirBuilder::new().mode(0o700).create(&ledger_path).unwrap();
        let base=meter::reset();
        let mut first=if case=="no-reasoning"{fixture::text_events("synthetic-smoke",fixture::known_usage())}else{fixture_driver::first_events()};
        match case.as_str() {
            "extra-text"=>{let text="synthetic-smoke-extra";first[4]["delta"]=json!(text);first[5]["text"]=json!(text);first[6]["part"]["text"]=json!(text);first[7]["item"]["content"][0]["text"]=json!(text);first[9]["response"]["output"][1]["content"][0]["text"]=json!(text);},
            "over-usage"=>{first[9]["response"]["usage"]["output_tokens"]=json!(1025);first[9]["response"]["usage"]["total_tokens"]=json!(1035);},
            "missing-usage"=>first[9]["response"]["usage"]=serde_json::Value::Null,
            "partial-usage"=>first[9]["response"]["usage"]=json!({"input_tokens":10,"input_tokens_details":{"cached_tokens":3}}),
            "unsupported"=>first[9]["response"]["output"][0]["opaque"]=json!("different"),
            "non-fit"|"nonfit-max"=>{let encrypted=json!("é".repeat(7800));first[8]["item"]["encrypted_content"]=encrypted.clone();first[9]["response"]["output"][0]["encrypted_content"]=encrypted;},
            "success-max"=>{let encrypted=json!(format!("{}{}","é".repeat(7400),"\u{0000}".repeat(64)));first[8]["item"]["encrypted_content"]=encrypted.clone();first[9]["response"]["output"][0]["encrypted_content"]=encrypted;},
            "evidence-budget"=>{let id=json!("r".repeat(15000));first[0]["response"]["id"]=id.clone();first[9]["response"]["id"]=id;},
            _=>{}
        }
        if case=="extra-message" {
            let mut terminal=first.pop().unwrap();let extra=fixture::text_events("extra",fixture::known_usage());
            for event in &extra[2..8] {let mut event=event.clone();event["output_index"]=json!(2);if event.get("item_id").is_some(){event["item_id"]=json!("extra-message");}
            if event.get("item").is_some(){event["item"]["id"]=json!("extra-message");}first.push(event);}
            let mut item=extra[7]["item"].clone();item["id"]=json!("extra-message");terminal["response"]["output"].as_array_mut().unwrap().push(item);first.push(terminal);
        }
        let mut body=fixture::wire(&first,true);
        if case=="trailing" {body.extend_from_slice(b"data: {}\n\n");}
        let mut changed=fixture_driver::metadata();changed[0]["model_catalog"]["new-predicate"]=json!(true);
        let second=fixture::call_events(if case=="bad-call"{"{\"message\":\"wrong\"}"}else{"{\"message\":\"synthetic-smoke\"}"},fixture::known_usage());
        let mut metadata=fixture_driver::metadata();
        if case.ends_with("max") {
            for value in &mut metadata {value["synthetic-padding"]=json!("");let length=serde_json::to_vec(value).unwrap().len();value["synthetic-padding"]=json!("x".repeat(130000-length));}
            let marker=b"data: [DONE]\r\n\r\n";
            let mut remaining=130000-body.len()-marker.len();
            while remaining>0 {let size=remaining.min(16384);let size=if remaining>size && remaining-size<3{size-3}else{size};body.push(b':');body.extend(std::iter::repeat_n(b'x',size-3));body.extend_from_slice(b"\n\n");remaining-=size;}
            body.extend_from_slice(marker);
        }
        let stopped=Arc::new(AtomicBool::new(case=="cancel"));
        let hold=case=="cancel-before-eof" || case=="deadline-before-eof";
        let (endpoint,server)=fixture_driver::server_controlled([body,fixture::wire(&second,false)],metadata,(case=="metadata-change").then_some(changed),hold,(case=="cancel-before-eof").then_some(stopped.clone()),hold||case=="partial-body").await;
        let mut expected=fixture_driver::seed(&ledger_path,endpoint).unwrap();
        if case.ends_with("max") {
            let file=ledger_path.join("provider-attempts.jsonl");let original=std::fs::read(&file).unwrap();let mut padded=Vec::new();
            for line in original.split(|b|*b==b'\n').filter(|line|!line.is_empty()){padded.extend(std::iter::repeat_n(b' ',17408-line.len()));padded.extend_from_slice(line);padded.push(b'\n');}
            std::fs::write(&file,&padded).unwrap();expected=hash(&padded);
        }
        let baseline=std::fs::read(ledger_path.join("provider-attempts.jsonl")).unwrap();
        let report=run_pair(endpoint,ledger_path.clone(),expected,root.join("evidence"),stopped,false).await.unwrap();
        let report:serde_json::Value=serde_json::from_slice(&report).unwrap();
        let observed=server.await.unwrap();
        if case.starts_with("success") || case=="no-reasoning" {
            assert_eq!(report["success"],true);assert_eq!(report["consumed"],6);assert_eq!(report["unresolved_remote"],4);assert_eq!(report["unknown_usage"],4);assert_eq!(observed.requests.len(),2);
            assert!(observed.dispatched[1].duration_since(observed.dispatched[0])>=Duration::from_secs(1));
            let mut expected_first:serde_json::Value=serde_json::from_str(&ProviderRequest::new(&[ProviderInput::User(TEXT_PROMPT.into())],&[]).unwrap().encoded).unwrap();
            assert_eq!(observed.requests[0],expected_first);
            let mut input=vec![json!({"role":"user","content":TEXT_PROMPT})];input.extend(first.last().unwrap()["response"]["output"].as_array().unwrap().iter().cloned());input.push(json!({"role":"user","content":CALL_PROMPT}));expected_first["input"]=json!(input);
            expected_first["tools"]=json!([{"type":"function","name":"inert","description":tools()[0].description,"parameters":tools()[0].parameters,"strict":true}]);
            assert_eq!(observed.requests[1],expected_first);
            println!("pair-request-bytes: case={case} first={} second={}",serde_json::to_vec(&observed.requests[0]).unwrap().len(),serde_json::to_vec(&observed.requests[1]).unwrap().len());
            assert_eq!(std::fs::read(root.join("evidence/second/request.json")).unwrap(),serde_json::to_vec(&observed.requests[1]).unwrap());
        } else {
            assert_eq!(report["success"],false,"{case}");assert_eq!(observed.requests.len(),if case=="bad-call"{2}else{usize::from(case!="cancel")},"{case}");assert_eq!(report["consumed"],if case=="cancel"{4}else if case=="bad-call"{6}else{5});
        }
        if matches!(case.as_str(),"cancel-before-eof"|"deadline-before-eof"|"partial-body") {
            let diagnostic:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("evidence/first/diagnostic.json")).unwrap()).unwrap();
            assert_eq!(diagnostic["outcome"],if case=="cancel-before-eof"{"Cancelled"}else if case=="deadline-before-eof"{"Deadline"}else{"Incomplete"});
            let provenance:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("evidence/first/provenance.json")).unwrap()).unwrap();assert_eq!(provenance["attempt"]["body_eof"],false);assert!(provenance["attempt"]["terminal"].is_null());assert!(provenance["attempt"]["usage"].is_null());assert_eq!(report["new_uncertainty"],true);
        }
        if case=="partial-usage" {
            let provenance:serde_json::Value=serde_json::from_slice(&std::fs::read(root.join("evidence/first/provenance.json")).unwrap()).unwrap();assert_eq!(provenance["attempt"]["usage"]["input_tokens"],10);assert!(provenance["attempt"]["usage"]["output_tokens"].is_null());assert_eq!(report["unresolved_remote"],4);assert_eq!(report["unknown_usage"],5);
        }
        if case=="extra-message" || case=="extra-text" {assert!(report["stop"].as_str().unwrap().contains("aggregate text"));}
        if case=="non-fit" || case=="nonfit-max" {assert!(report["stop"].as_str().unwrap().contains("context resource"));}
        meter::check(base,&case);
        let after=std::fs::read(ledger_path.join("provider-attempts.jsonl")).unwrap();assert!(after.starts_with(&baseline));
        let private_summary=std::fs::read(root.join("evidence/summary.json")).unwrap();assert!(private_summary.len()<=4096);
        if case=="evidence-budget" {assert!(std::fs::metadata(root.join("evidence/first/response.data")).unwrap().len()>15000);assert_eq!(report["accounting_observed"],true);assert_eq!(report["summary_durable"],true);assert_eq!(report["consumed"],5);}
        let checkpoint:ValidationCheckpoint=serde_json::from_slice(&std::fs::read(root.join("evidence/checkpoint.json")).unwrap()).unwrap();
        assert_eq!(checkpoint.sha256,expected);
        assert_eq!(std::fs::read(ledger_path.join("provider-attempts.jsonl")).unwrap(),after);
        println!("PAIR_RECOVERY|{}",root.display());
    });
}
