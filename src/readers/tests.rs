use super::*;
#[path = "../../tests/support/readers_root.rs"]
pub(super) mod support;

#[test]
fn strict_schema_and_serialization_are_bounded_and_order_independent() {
    let valid = r#"{"summary":"fine","child":0,"generation":1,"session":"schema"}"#;
    assert_eq!(decode(valid, "schema", 1, 0).unwrap(), "fine");
    for text in [
        "[]",
        "{}",
        r#"["schema",1,0,"fine"]"#,
        r#"{"session":"schema","generation":1,"child":0,"summary":[]}"#,
        r#"{"session":"schema","generation":1,"child":0,"summary":"fine","x":{}}"#,
        r#"{"session":"schema","generation":1,"child":0,"summary":"fine","child":0}"#,
        r#"{"session":"schema","generation":1,"child":0,"summary":"fine"} {}"#,
        r#"{"session":"schema","generation":1.0,"child":0,"summary":"fine"}"#,
        r#"{"session":"schema","generation":1,"child":3,"summary":"fine"}"#,
        r#"{"session":"schema","generation":1,"child":0,"summary":"\n"}"#,
    ] {
        assert!(decode(text, "schema", 1, 0).is_err(), "{text}");
    }
    let inputs = ReaderInputs::new(
        "s",
        u64::MAX,
        [&"\t".repeat(1024), &"界".repeat(341), &"\\\"".repeat(512)],
    )
    .unwrap();
    for i in 0..3 {
        let text = context(&inputs, i).unwrap();
        assert_eq!(text.capacity(), 4096);
        assert!(text.len() <= 4096);
    }
    let mut backing = String::with_capacity(4 * 1024 * 1024);
    backing.push('x');
    let inputs = ReaderInputs::new("s", 1, [&backing, "", ""]).unwrap();
    assert_eq!(inputs.contents[0].len(), 1);
    assert_ne!(inputs.contents[0].as_ptr(), backing.as_ptr());
    assert!(Arc::ptr_eq(
        &inputs.contents[0],
        &inputs.clone().contents[0]
    ));
    let huge = format!(
        r#"{{"summary":"{}","session":"schema","generation":1,"child":0}}"#,
        "x".repeat(513)
    );
    assert!(decode(&huge, "schema", 1, 0).is_err());
}

#[test]
fn terminal_acceptance_cancellation_and_storage_poison_are_truthful() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let root = support::OwnedRoot::new();
    let inputs = ReaderInputs::new("worker", 1, [""; 3]).unwrap();
    let mut journal = journal::Journal::create(&root.path, &inputs).unwrap();
    journal.append(&journal::Record::Started(0)).unwrap();
    let control = Arc::new(Control::new("worker", 1));
    control
        .publish(|state| state.submitted(0, RequestId(10)))
        .unwrap();
    cancel(&control, Some(0));
    let (sender, receiver) = mpsc::channel(1);
    let (reply, receive) = oneshot::channel();
    sender
        .try_send(Work {
            record: journal::Record::Terminal {
                child: 0,
                id: Some(RequestId(10)),
                state: ReaderState::Completed {
                    summary: "late".to_owned(),
                },
                response: r#"{"session":"worker","generation":1,"child":0,"summary":"late"}"#
                    .to_owned(),
            },
            reply,
        })
        .unwrap();
    drop(sender);
    journal_worker(journal, receiver, control.clone());
    assert_eq!(
        receive.blocking_recv().unwrap().unwrap().unwrap().state,
        ReaderState::Cancelled
    );
    assert_eq!(
        journal::reconstruct(&root.path, "worker", 1)
            .unwrap()
            .children[0]
            .state,
        ReaderState::Cancelled
    );
    assert!(control.state.lock().unwrap().accepted[0]);
}

#[test]
fn cancellation_during_sync_persists_correction_before_live_acceptance() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let root = support::OwnedRoot::new();
    let mut journal =
        journal::Journal::create(&root.path, &ReaderInputs::new("sync", 1, [""; 3]).unwrap())
            .unwrap();
    journal.append(&journal::Record::Started(0)).unwrap();
    let (reached, waiting) = std::sync::mpsc::sync_channel(1);
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    journal.gate = Some((reached, blocked));
    let control = Arc::new(Control::new("sync", 1));
    control
        .publish(|state| state.submitted(0, RequestId(11)))
        .unwrap();
    let worker_control = control.clone();
    let (sender, receiver) = mpsc::channel(1);
    let worker = std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
    let (reply, receive) = oneshot::channel();
    sender
        .try_send(Work {
            record: journal::Record::Terminal {
                child: 0,
                id: Some(RequestId(11)),
                state: ReaderState::Completed {
                    summary: "late".to_owned(),
                },
                response: r#"{"session":"sync","generation":1,"child":0,"summary":"late"}"#
                    .to_owned(),
            },
            reply,
        })
        .unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    cancel(&control, Some(0));
    assert!(!control.state.lock().unwrap().accepted[0]);
    assert!(control.observation.borrow().children[0].fact.is_none());
    release.send(()).unwrap();
    assert_eq!(
        receive.blocking_recv().unwrap().unwrap().unwrap().state,
        ReaderState::Cancelled
    );
    drop(sender);
    worker.join().unwrap();
    let tree = journal::reconstruct(&root.path, "sync", 1).unwrap();
    assert!(!tree.parent_finished);
    assert_eq!(tree.children[0].state, ReaderState::Cancelled);
}

#[test]
fn cancellation_after_acceptance_freeze_keeps_live_and_durable_completion() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let root = support::OwnedRoot::new();
    let mut journal = journal::Journal::create(
        &root.path,
        &ReaderInputs::new("frozen", 1, [""; 3]).unwrap(),
    )
    .unwrap();
    journal.append(&journal::Record::Started(0)).unwrap();
    let (reached, waiting) = std::sync::mpsc::sync_channel(1);
    let (release, blocked) = std::sync::mpsc::sync_channel(1);
    journal.accept_gate = Some((reached, blocked));
    let control = Arc::new(Control::new("frozen", 1));
    control
        .publish(|state| state.submitted(0, RequestId(11)))
        .unwrap();
    let worker_control = control.clone();
    let (sender, receiver) = mpsc::channel(1);
    let worker = std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
    let (reply, mut receive) = oneshot::channel();
    sender
        .try_send(Work {
            record: journal::Record::Terminal {
                child: 0,
                id: Some(RequestId(11)),
                state: ReaderState::Completed {
                    summary: "frozen".to_owned(),
                },
                response: r#"{"session":"frozen","generation":1,"child":0,"summary":"frozen"}"#
                    .to_owned(),
            },
            reply,
        })
        .unwrap();
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(control.state.lock().unwrap().accepted[0]);
    assert!(control.observation.borrow().children[0].fact.is_none());
    assert!(matches!(
        receive.try_recv(),
        Err(oneshot::error::TryRecvError::Empty)
    ));
    cancel(&control, None);
    assert_eq!(control.state.lock().unwrap().cancelled, [false, true, true]);
    release.send(()).unwrap();
    let live = receive.blocking_recv().unwrap().unwrap().unwrap();
    drop(sender);
    worker.join().unwrap();
    assert_eq!(
        control.observation.borrow().children[0].fact.as_ref(),
        Some(&live)
    );
    assert_eq!(
        control.observation.borrow().children[0].acceptance_ordinal,
        Some(1)
    );
    assert_eq!(
        live.state,
        ReaderState::Completed {
            summary: "frozen".to_owned()
        }
    );
    assert_eq!(
        journal::reconstruct(&root.path, "frozen", 1)
            .unwrap()
            .children[0],
        live
    );
}

#[test]
fn acceptance_write_failures_cancel_and_join_actual_child_tree() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            for fault in [
                journal::Fault::Partial,
                journal::Fault::Append,
                journal::Fault::Sync,
            ] {
                let root = support::OwnedRoot::new();
                let fixture = fixture::ReadersFixture::start(fixture::golden("ackfault", 1)).await;
                let runtime = Runtime::new(
                    fixture.endpoint,
                    2,
                    crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    crate::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1),
                    )
                    .unwrap(),
                    crate::ConcurrencyMode::Fixed,
                )
                .unwrap();
                let inputs = ReaderInputs::new("ackfault", 1, ["a", "b", "c"]).unwrap();
                let mut journal = journal::Journal::create(&root.path, &inputs).unwrap();
                let (reached, waiting) = std::sync::mpsc::sync_channel(1);
                let (release, blocked) = std::sync::mpsc::sync_channel(1);
                let partial = matches!(fault, journal::Fault::Partial);
                let full = matches!(fault, journal::Fault::Sync);
                journal.accept_fault = Some(fault);
                journal.accept_gate = Some((reached, blocked));
                let control = Arc::new(Control::new("ackfault", 1));
                let worker_control = control.clone();
                let (sender, receiver) = mpsc::channel(1);
                let worker =
                    std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
                let coordinator = coordinate(&runtime, &inputs, &control, &sender);
                let checkpoint = async {
                    fixture.wait_child(0).await;
                    fixture.wait_child(1).await;
                    while runtime.snapshot().pending != 1 {
                        tokio::task::yield_now().await;
                    }
                    fixture.release(0);
                    tokio::time::timeout(Duration::from_secs(5), async {
                        while waiting.try_recv().is_err() {
                            tokio::task::yield_now().await;
                        }
                    })
                    .await
                    .unwrap();
                    assert!(control.state.lock().unwrap().accepted[0]);
                    assert_eq!(runtime.snapshot().active, 2);
                    assert!(
                        Runtime::new(
                            fixture.endpoint,
                            2,
                            crate::RequestRate::new(4, std::time::Duration::from_millis(1))
                                .unwrap(),
                            crate::SyntheticTokenBudget::new(
                                65_536,
                                65_536,
                                std::time::Duration::from_millis(1)
                            )
                            .unwrap(),
                            crate::ConcurrencyMode::Fixed,
                        )
                        .is_err()
                    );
                    release.send(()).unwrap();
                };
                let (result, ()) = tokio::join!(coordinator, checkpoint);
                assert!(result.is_err());
                assert!(
                    control
                        .observation
                        .borrow()
                        .children
                        .iter()
                        .all(|slot| slot.fact.is_none())
                );
                assert!(control.observation.borrow().error.is_some());
                assert_eq!(control.state.lock().unwrap().cancelled, [false, true, true]);
                assert_eq!(runtime.snapshot().handles, 0);
                drop(sender);
                tokio::task::spawn_blocking(move || worker.join())
                    .await
                    .unwrap()
                    .unwrap();
                let snapshot = runtime.shutdown().await;
                assert_eq!(
                    (snapshot.active, snapshot.pending, snapshot.handles),
                    (0, 0, 0)
                );
                let path = root.path.join(".reader-journal");
                let bytes = std::fs::read(&path).unwrap();
                let inode = rustix::fs::fstat(std::fs::File::open(&path).unwrap())
                    .unwrap()
                    .st_ino;
                let reconstructed = journal::reconstruct(&root.path, "ackfault", 1);
                if partial {
                    assert!(reconstructed.is_err());
                } else {
                    let tree = reconstructed.unwrap();
                    assert!(!tree.parent_finished);
                    assert_eq!(
                        tree.children[0].state,
                        if full {
                            ReaderState::Completed {
                                summary: "reader-0".to_owned(),
                            }
                        } else {
                            ReaderState::Interrupted
                        }
                    );
                    assert!(
                        tree.children[1..]
                            .iter()
                            .all(|f| f.state == ReaderState::Interrupted)
                    );
                }
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert_eq!(
                    rustix::fs::fstat(std::fs::File::open(&path).unwrap())
                        .unwrap()
                        .st_ino,
                    inode
                );
                fixture.shutdown().await;
                assert_eq!(Runtime::process_snapshot().unresolved, 0);
                Runtime::new(
                    "127.0.0.1:9".parse().unwrap(),
                    2,
                    crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                    crate::SyntheticTokenBudget::new(
                        65_536,
                        65_536,
                        std::time::Duration::from_millis(1),
                    )
                    .unwrap(),
                    crate::ConcurrencyMode::Fixed,
                )
                .unwrap()
                .shutdown()
                .await;
            }
        });
}

#[test]
fn same_accepted_cancelled_fact_keeps_actual_transport_accounting_distinct() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let _owner = crate::runtime::tests::TEST_OWNER.lock().await;
            for interrupted in [false, true] {
                tokio::time::sleep(Duration::from_millis(5)).await;
                let root = support::OwnedRoot::new();
                let fixture =
                    fixture::ReadersFixture::start(fixture::golden("accounted-cancel", 1)).await;
                let runtime = Runtime::new(
                    fixture.endpoint,
                    2,
                    crate::RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                    crate::SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1))
                        .unwrap(),
                    crate::ConcurrencyMode::Fixed,
                )
                .unwrap();
                let inputs = ReaderInputs::new("accounted-cancel", 1, [""; 3]).unwrap();
                let mut journal = journal::Journal::create(&root.path, &inputs).unwrap();
                journal.append(&journal::Record::Started(0)).unwrap();
                let (reached, waiting) = std::sync::mpsc::sync_channel(1);
                let (release, blocked) = std::sync::mpsc::sync_channel(1);
                journal.gate = Some((reached, blocked));
                let control = Arc::new(Control::new("accounted-cancel", 1));
                let observer = ReaderObserver::new(control.observation.subscribe());
                let (sender, receiver) = mpsc::channel(1);
                let worker_control = control.clone();
                let worker =
                    std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
                let mut handle = runtime
                    .submit(
                        &context(&inputs, 0).unwrap(),
                        crate::SyntheticCost::new(1, 1).unwrap(),
                    )
                    .unwrap();
                let id = handle.id();
                control.publish(|state| state.submitted(0, id)).unwrap();
                fixture.wait_child(0).await;
                if interrupted {
                    cancel(&control, Some(0));
                    handle.cancel();
                } else {
                    fixture.release(0);
                }
                let mut response = String::new();
                while let Some(delta) = handle.next_delta().await {
                    response.push_str(&delta);
                }
                let terminal = handle.terminal().await.unwrap();
                assert!(handle.terminal().await.is_none());
                assert_eq!(
                    terminal.outcome,
                    if interrupted {
                        Outcome::Cancelled
                    } else {
                        Outcome::Success
                    }
                );
                let (unknown_usage, unresolved_exposure) = handle.accounting_receipt().unwrap();
                assert_eq!(
                    (unknown_usage, unresolved_exposure),
                    (interrupted, interrupted)
                );
                control
                    .publish(|state| {
                        state.accounted(
                            0,
                            ReaderAccounting {
                                request_id: id,
                                unknown_usage,
                                unresolved_exposure,
                            },
                        )
                    })
                    .unwrap();
                drop(handle);
                let (reply, receive) = oneshot::channel();
                sender
                    .send(Work {
                        record: journal::Record::Terminal {
                            child: 0,
                            id: Some(id),
                            state: if interrupted {
                                ReaderState::Cancelled
                            } else {
                                ReaderState::Completed {
                                    summary: decode(&response, "accounted-cancel", 1, 0).unwrap(),
                                }
                            },
                            response,
                        },
                        reply,
                    })
                    .await
                    .unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while waiting.try_recv().is_err() {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
                assert!(observer.snapshot().children[0].fact.is_none());
                cancel(&control, Some(0));
                release.send(()).unwrap();
                let fact = receive.await.unwrap().unwrap().unwrap();
                assert_eq!(fact.state, ReaderState::Cancelled);
                assert_eq!(observer.snapshot().children[0].fact.as_ref(), Some(&fact));
                let receipt = observer.snapshot().children[0].accounting.unwrap();
                assert_eq!(
                    (receipt.unknown_usage, receipt.unresolved_exposure),
                    (interrupted, interrupted)
                );
                drop(sender);
                tokio::task::spawn_blocking(move || worker.join())
                    .await
                    .unwrap()
                    .unwrap();
                let snapshot = runtime.shutdown().await;
                assert_eq!(snapshot.unknown_usage, usize::from(interrupted));
                assert_eq!(snapshot.unresolved, usize::from(interrupted));
                assert_eq!(
                    reconstruct_readers(&root.path, "accounted-cancel", 1)
                        .unwrap()
                        .children[0],
                    fact
                );
                fixture.shutdown().await;
            }
        });
}

#[test]
fn lost_supervisor_is_bounded_and_never_fabricates_accounting_or_acceptance() {
    let _gate = crate::coding::tests::PROCESS_GATE.write().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "readers::tests::lost_supervisor_child_helper",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("unjoined runtime retained; no fabricated facts")
    );
}

#[test]
#[ignore = "isolated intentional lost-supervisor accounting failure"]
fn lost_supervisor_child_helper() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let root = support::OwnedRoot::new();
            let runtime = Runtime::new(
                "127.0.0.1:9".parse().unwrap(),
                2,
                crate::RequestRate::new(4, Duration::from_millis(1)).unwrap(),
                crate::SyntheticTokenBudget::new(65536, 65536, Duration::from_millis(1)).unwrap(),
                crate::ConcurrencyMode::Fixed,
            )
            .unwrap();
            crate::runtime::tests::abort_supervisor(&runtime);
            let (parent, cleanup) = ReaderParent::start(
                runtime,
                &root.path,
                ReaderInputs::new("lost-supervisor", 1, [""; 3]).unwrap(),
            )
            .unwrap();
            let observer = parent.observer();
            let control = parent.control();
            let finish = tokio::spawn(parent.finish());
            tokio::time::timeout(Duration::from_secs(3), async {
                while observer
                    .snapshot()
                    .children
                    .iter()
                    .any(|slot| slot.request_id.is_none())
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            control.cancel_tree("lost-supervisor", 1).unwrap();
            let start = std::time::Instant::now();
            assert!(
                tokio::time::timeout(Duration::from_secs(8), finish)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert!(cleanup.wait().await.is_err());
            assert!(start.elapsed() < Duration::from_secs(8));
            let observation = observer.snapshot();
            assert!(
                observation.storage_joined
                    && !observation.runtime_joined
                    && observation.error.is_some()
            );
            assert!(
                observation
                    .children
                    .iter()
                    .all(|slot| slot.fact.is_none() && slot.accounting.is_none())
            );
            assert!(Runtime::process_snapshot().active > 0);
            println!("unjoined runtime retained; no fabricated facts");
        });
}

#[test]
fn earlier_acknowledged_siblings_survive_later_accept_and_end_capacity_failure() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    for end_failure in [false, true] {
        let root = support::OwnedRoot::new();
        let inputs = ReaderInputs::new("retained-failure", 1, [""; 3]).unwrap();
        let mut journal = journal::Journal::create(&root.path, &inputs).unwrap();
        journal.limits = (if end_failure { 10 } else { 6 }, 32768);
        let control = Arc::new(Control::new("retained-failure", 1));
        let observer = ReaderObserver::new(control.observation.subscribe());
        let (sender, receiver) = mpsc::channel(1);
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
        let append = |record| {
            let (reply, receive) = oneshot::channel();
            sender.blocking_send(Work { record, reply }).unwrap();
            receive.blocking_recv().unwrap()
        };
        for child in 0..if end_failure { 3 } else { 2 } {
            append(journal::Record::Started(child)).unwrap();
            control
                .publish(|state| state.submitted(child, RequestId(child as u64 + 100)))
                .unwrap();
        }
        let terminal = |child| journal::Record::Terminal {
            child,
            id: Some(RequestId(child as u64 + 100)),
            state: ReaderState::Completed {
                summary: format!("reader-{child}"),
            },
            response: format!(
                r#"{{"session":"retained-failure","generation":1,"child":{child},"summary":"reader-{child}"}}"#
            ),
        };
        let first = append(terminal(0)).unwrap().unwrap();
        assert_eq!(observer.snapshot().children[0].fact.as_ref(), Some(&first));
        if end_failure {
            append(terminal(1)).unwrap();
            append(terminal(2)).unwrap();
            assert!(append(journal::Record::End(0)).is_err());
        } else {
            assert!(append(terminal(1)).is_err());
            assert!(observer.snapshot().children[1].fact.is_none());
        }
        let observation = observer.snapshot();
        assert_eq!(observation.children[0].fact.as_ref(), Some(&first));
        assert_eq!(observation.children[0].acceptance_ordinal, Some(1));
        assert!(!observation.tree_ended && observation.error.is_some());
        drop(sender);
        worker.join().unwrap();
        let bytes = std::fs::read(root.path.join(".reader-journal")).unwrap();
        let tree = reconstruct_readers(&root.path, "retained-failure", 1).unwrap();
        assert_eq!(tree.children[0], first);
        assert!(!tree.parent_finished);
        assert_eq!(
            std::fs::read(root.path.join(".reader-journal")).unwrap(),
            bytes
        );
    }
}

#[path = "../../tests/support/readers_fixture.rs"]
mod fixture;
#[test]
#[ignore = "explicit fresh-process actual-HTTP checkpoint writer"]
fn actual_admitted_child_completion_exit() {
    let _gate = crate::coding::tests::PROCESS_GATE.read().unwrap();
    let Ok(path) = std::env::var("SCRAMJET_READER_RESTART_ROOT") else {
        return;
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let fixture = fixture::ReadersFixture::start(fixture::golden("restart", 1)).await;
            let runtime = Runtime::new(
                fixture.endpoint,
                2,
                crate::RequestRate::new(4, std::time::Duration::from_millis(1)).unwrap(),
                crate::SyntheticTokenBudget::new(
                    65_536,
                    65_536,
                    std::time::Duration::from_millis(1),
                )
                .unwrap(),
                crate::ConcurrencyMode::Fixed,
            )
            .unwrap();
            let inputs = ReaderInputs::new("restart", 1, ["a", "b", "c"]).unwrap();
            let journal = journal::Journal::create(Path::new(&path), &inputs).unwrap();
            let control = Arc::new(Control::new("restart", 1));
            let (sender, receiver) = mpsc::channel(1);
            let worker_control = control.clone();
            let _worker =
                std::thread::spawn(move || journal_worker(journal, receiver, worker_control));
            let first = child(&runtime, &inputs, 0, &control, &sender);
            let second = child(&runtime, &inputs, 1, &control, &sender);
            let checkpoint = async {
                fixture.wait_child(0).await;
                fixture.wait_child(1).await;
                assert_eq!(fixture.peak(), 2);
                assert!(fixture.contexts()[2].is_none());
                fixture.release(0);
            };
            let complete = async {
                let (fact, _) = first.await.unwrap();
                assert_eq!(
                    fact.state,
                    ReaderState::Completed {
                        summary: "reader-0".to_owned()
                    }
                );
                std::process::exit(17);
            };
            tokio::join!(complete, second, checkpoint);
            fixture.release_all();
            fixture.shutdown().await;
        });
}
