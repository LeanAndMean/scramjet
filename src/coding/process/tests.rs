use super::*;
fn captures(script: &str) -> Vec<CapturedFile> {
    vec![
        CapturedFile {
            path: "verse notes hé.md".into(),
            identity: [1, 2],
            content: "# Verse\r\nQuiet hé🙂\rFiles\0sing\n".into(),
        },
        CapturedFile {
            path: "reader alternate.py".into(),
            identity: [1, 3],
            content: script.into(),
        },
    ]
}
fn run(script: &str) -> ProcessResult {
    assert!(script.len() <= 512);
    python(
        &captures(script),
        "reader alternate.py",
        &EditCancellation::default(),
        |_, _| Ok(()),
    )
    .unwrap()
}
#[test]
fn actual_direct_python_preserves_selected_bytes_and_binary_output() {
    let result = run(
        "import sys\nwith open('verse notes hé.md','rb') as source:\n sys.stdout.buffer.write(source.read())\n",
    );
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(result.stopped.is_none());
    assert_eq!(
        result.stdout.text().unwrap(),
        "# Verse\r\nQuiet hé🙂\rFiles\0sing\n"
    );
    assert_eq!(result.stderr.text(), Some(""));
    assert!(result.launcher_reaped && result.namespace_dead);
    let result = run(
        "import sys\nsys.stdout.buffer.write(bytes([255,0,254]))\nsys.stderr.buffer.write(bytes([253]))\nsys.exit(3)\n",
    );
    assert_eq!(result.exit_code, Some(3));
    assert!(result.stopped.is_none());
    assert_eq!(
        result.stdout,
        ProcessOutput::Bytes {
            bytes: vec![255, 0, 254]
        }
    );
    assert_eq!(result.stderr, ProcessOutput::Bytes { bytes: vec![253] });
    assert_eq!(
        serde_json::from_slice::<ProcessResult>(&serde_json::to_vec(&result).unwrap()).unwrap(),
        result
    );
}
#[test]
fn zero_writable_scratch_and_no_spawn_network_or_backing_storage() {
    for script in [
        "import os\nfor p in ['/tmp/new','/new','/workspace/new','/dev/shm/new']:\n try:\n  open(p,'w').write('x'); print('BAD')\n except OSError as e: print(e.errno)\n",
        "import os,socket\nfor f in [os.fork,os.pipe,lambda: socket.socket(),lambda: os.memfd_create('small')]:\n try:\n  f();print('BAD')\n except OSError as e:print(e.errno)\n",
        "import ctypes\nc=ctypes.CDLL(None,use_errno=True)\nprint(c.syscall(29,0,1,0),ctypes.get_errno())\n",
    ] {
        let result = run(script);
        assert_eq!(result.exit_code, Some(0), "{result:?}");
        assert_eq!(result.stderr.text(), Some(""));
        let output = result.stdout.text().unwrap();
        assert!(!output.contains("BAD"), "{result:?}");
        if script.contains("syscall") {
            assert_eq!(output, "-1 1\n");
        } else if script.contains("/tmp/") {
            assert_eq!(output, "30\n30\n30\n30\n");
        } else {
            assert_eq!(output, "1\n1\n1\n1\n");
        }
        assert!(result.launcher_reaped && result.namespace_dead);
    }
}
#[test]
fn direct_profile_has_no_host_paths_shell_or_leaked_descriptors() {
    let _inheritable = rustix::fs::open(
        "/dev/null",
        rustix::fs::OFlags::RDONLY,
        rustix::fs::Mode::empty(),
    )
    .unwrap();
    let result = run(
        "import os,json,resource\nprint(json.dumps({'fds':os.listdir('/proc/self/fd'),'init':os.listdir('/proc/1/fd'),'paths':[os.path.exists(p) for p in ['/home','/root','/etc','/bin/bash','/workspace/storage']], 'env':dict(os.environ),'limits':[resource.getrlimit(r) for r in [resource.RLIMIT_CPU,resource.RLIMIT_AS,resource.RLIMIT_NOFILE,resource.RLIMIT_CORE]]}))\n",
    );
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    let observed: serde_json::Value = serde_json::from_str(result.stdout.text().unwrap()).unwrap();
    assert_eq!(
        observed["paths"],
        serde_json::json!([false, false, false, false, false])
    );
    for key in ["fds"] {
        for fd in observed[key].as_array().unwrap() {
            assert!(
                fd.as_str().unwrap().parse::<usize>().unwrap() <= 3,
                "{result:?}"
            );
        }
    }
    assert_eq!(
        observed["limits"],
        serde_json::json!([[2, 2], [268435456, 268435456], [32, 32], [0, 0]])
    );
    let init_probe = run(
        "import os,json\nprint(json.dumps({x:os.readlink('/proc/1/fd/'+x) for x in os.listdir('/proc/1/fd')}))\n",
    );
    let init: serde_json::Value = serde_json::from_str(init_probe.stdout.text().unwrap()).unwrap();
    let init = init.as_object().unwrap();
    assert_eq!(init.len(), 4, "{init_probe:?}");
    assert_eq!(init["0"], "/dev/null");
    assert!(init["1"].as_str().unwrap().starts_with("pipe:["));
    assert!(init["2"].as_str().unwrap().starts_with("pipe:["));
    assert_eq!(
        init.iter()
            .find(|(fd, _)| !["0", "1", "2"].contains(&fd.as_str()))
            .unwrap()
            .1,
        "anon_inode:[eventfd]"
    );
    assert_eq!(observed["env"]["PATH"], "/usr/bin:/bin");
    assert!(
        observed["env"]
            .as_object()
            .unwrap()
            .keys()
            .all(|key| ["PATH", "PWD", "LC_CTYPE"].contains(&key.as_str()))
    );
}
#[test]
fn generated_profile_output_memory_fd_cpu_wall_and_cancellation_settle() {
    for script in [
        "import sys\nsys.stdout.write('x'*20000)\n",
        "import time\ntime.sleep(10)\n",
    ] {
        let result = run(script);
        assert!(result.stopped.is_some(), "{result:?}");
        assert!(result.launcher_reaped && result.namespace_dead);
        assert!(result.stdout.byte_len() + result.stderr.byte_len() <= 16384);
    }
    for (script, expected) in [
        ("x=bytearray(400*1024*1024)\n", "MemoryError"),
        (
            "import os\nf=[os.open('verse notes hé.md',os.O_RDONLY) for _ in range(100)]\n",
            "Too many open files",
        ),
    ] {
        let result = run(script);
        assert_eq!(result.exit_code, Some(1), "{result:?}");
        assert!(
            result.stderr.text().unwrap().contains(expected),
            "{expected}: {result:?}"
        );
        assert!(result.launcher_reaped && result.namespace_dead);
    }
    let result = run("while True: pass\n");
    assert_eq!(result.exit_code, None, "{result:?}");
    assert_eq!(result.signal, Some(9), "{result:?}");
    assert!(result.launcher_reaped && result.namespace_dead);
    let cancellation = EditCancellation::default();
    let cancel = cancellation.clone();
    let signal = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        cancel.cancel();
    });
    let result = python(
        &captures("import time\ntime.sleep(10)\n"),
        "reader alternate.py",
        &cancellation,
        |_, _| Ok(()),
    )
    .unwrap();
    signal.join().unwrap();
    assert!(result.stopped.is_some());
    assert!(result.launcher_reaped && result.namespace_dead);
}
#[test]
fn alternate_syscall_abis_and_writable_proc_are_not_capabilities() {
    for script in [
        "import ctypes\nctypes.CDLL(None).syscall(0x40000027)\nprint('MUST NOT RUN')\n",
        "import mmap,ctypes\nm=mmap.mmap(-1,4096,prot=7)\nm.write(bytes([184,20,0,0,0,205,128,195]))\nctypes.CFUNCTYPE(ctypes.c_int)(ctypes.addressof(ctypes.c_char.from_buffer(m)))()\nprint('MUST NOT RUN')\n",
    ] {
        let result = run(script);
        assert_eq!(result.exit_code, None, "{result:?}");
        assert_eq!(result.signal, Some(31), "{result:?}");
        assert_eq!(result.stdout.text(), Some(""));
        assert!(result.launcher_reaped && result.namespace_dead);
    }
    let result = run(
        "import os\nfor p in ['/proc/self/mem','/proc/1/mem','/proc/1/oom_score_adj','/proc/sys/kernel/ns_last_pid']:\n try:\n  f=os.open(p,os.O_RDWR);os.close(f);print('BAD')\n except OSError as e: print(e.errno)\n",
    );
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert!(!result.stdout.text().unwrap().contains("BAD"), "{result:?}");
    assert!(result.launcher_reaped && result.namespace_dead);
}
#[test]
fn self_stop_cannot_silently_execute_following_instructions() {
    let result = run(
        "import os,signal\nprint('before stop',flush=True)\nos.kill(os.getpid(),signal.SIGSTOP)\nprint('MUST NOT RUN')\n",
    );
    assert_eq!(result.stdout.text(), Some("before stop\n"));
    assert_eq!(
        result.stopped.as_deref(),
        Some("isolated job-control stop unsupported")
    );
    assert!(result.launcher_reaped && result.namespace_dead);
}
#[test]
fn generated_supervisor_reclaims_while_start_acknowledgement_is_blocked() {
    use rustix::{
        event::{PollFd, PollFlags, poll},
        process::{Pid, PidfdFlags, pidfd_open},
    };
    let result = python(
        &captures("print('MUST NOT RUN')\n"),
        "reader alternate.py",
        &EditCancellation::default(),
        |launcher, init| {
            let handles = [launcher, init].map(|pid| {
                pidfd_open(Pid::from_raw(pid as i32).unwrap(), PidfdFlags::empty()).unwrap()
            });
            std::thread::sleep(std::time::Duration::from_millis(5300));
            for handle in handles {
                let mut fds = [PollFd::new(&handle, PollFlags::IN)];
                poll(
                    &mut fds,
                    Some(&rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: 0,
                    }),
                )
                .unwrap();
                assert!(
                    fds[0].revents().contains(PollFlags::IN),
                    "namespace/launcher survived blocked acknowledgement"
                );
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(result.stopped.is_some());
    assert_eq!(result.stdout.text(), Some(""));
    assert!(result.launcher_reaped && result.namespace_dead);
}
#[test]
fn generated_command_cannot_pass_start_acknowledgement_refusal() {
    let mut observed = 0;
    let result = python(
        &captures("print('MUST NOT RUN')\n"),
        "reader alternate.py",
        &EditCancellation::default(),
        |_, _| {
            observed += 1;
            Err(CodingError::Poisoned)
        },
    );
    assert!(matches!(result, Err(CodingError::Poisoned)));
    assert_eq!(observed, 1);
}
