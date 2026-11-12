//! macOS's counterpart of bubblewrap: profiles for `sandbox-exec` (Seatbelt), matching Codex's
//! (`codex-rs/sandboxing/src/seatbelt*.rs`, read 2026-11-07): closed by default, every file
//! readable, writes only where allowed, the network and Unix sockets only when it is on. Paths go
//! to the profile as parameters (`-D`), never inside its text. Rules that come later win, so the
//! denials come last. Not yet run on a Mac.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

/// Only the system's own, as Codex takes it: one earlier in `PATH` could be anything.
pub(super) const PROGRAM: &str = "/usr/bin/sandbox-exec";

/// What a closed profile lets processes do besides reading files: run programs, signal and
/// inspect each other, the system queries common tools make (user lookups, preferences,
/// semaphores, `/dev/null`, pseudo-terminals they open themselves).
const BASE: &str = r#"(version 1)
(deny default)
(allow process-exec process-fork)
(allow signal (target same-sandbox))
(allow process-info* (target same-sandbox))
(allow sysctl-read)
(allow file-read*)
(allow file-write-data (require-all (path "/dev/null") (vnode-type CHARACTER-DEVICE)))
(allow pseudo-tty)
(allow file-read* file-write* file-ioctl (literal "/dev/ptmx"))
(allow file-read* file-write* file-ioctl
  (require-all (regex #"^/dev/ttys[0-9]+") (extension "com.apple.sandbox.pty")))
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.PowerManagement.control")
  (global-name "com.apple.cfprefsd.daemon")
  (global-name "com.apple.cfprefsd.agent")
  (local-name "com.apple.cfprefsd.agent"))
(allow user-preference-read)
(allow ipc-posix-shm-read* (ipc-posix-name-prefix "apple.cfprefs."))
(allow ipc-posix-sem)
"#;

/// The network, with the services that name resolution and TLS certificates go through.
const NETWORK: &str = r#"(allow network-outbound network-inbound network-bind)
(allow system-socket (require-all (socket-domain AF_SYSTEM) (socket-protocol 2)))
(allow mach-lookup
  (global-name "com.apple.bsd.dirhelper")
  (global-name "com.apple.system.opendirectoryd.membership")
  (global-name "com.apple.SecurityServer")
  (global-name "com.apple.networkd")
  (global-name "com.apple.ocspd")
  (global-name "com.apple.trustd.agent")
  (global-name "com.apple.TrustEvaluationAgent")
  (global-name "com.apple.SystemConfiguration.DNSConfiguration")
  (global-name "com.apple.SystemConfiguration.configd"))
(allow sysctl-read (sysctl-name-regex #"^net.routetable"))
"#;

/// The closing denials: no XPC service, and not the two `fcntl`s that change a file through a
/// descriptor opened read-only (`F_MAKECOMPRESSED`, `F_TRANSFEREXTENTS`).
const CLOSED: &str = r#"(deny mach-lookup (xpc-service-name-prefix ""))
(deny system-fcntl (fcntl-command 80 110))
"#;

/// A profile's text and the paths it names.
#[derive(Default)]
struct Profile {
    text: String,
    params: Vec<PathBuf>,
}

impl Profile {
    /// A filter naming `path` as `literal` or `subpath`.
    fn path(&mut self, kind: &str, path: &Path) -> String {
        self.params.push(path.to_owned());
        format!("({kind} (param \"P{}\"))", self.params.len() - 1)
    }

    fn rule(&mut self, rule: &str) {
        self.text.push_str(rule);
        self.text.push('\n');
    }

    /// Hides `files` and `directories` from reads and writes, and the Unix sockets in the
    /// directories (tmux's) from connections, with the directories above each kept from being
    /// renamed, which would move it out of its path.
    fn hide(&mut self, files: &[&PathBuf], directories: &[&PathBuf]) {
        for file in files {
            let filter = self.path("literal", file);
            self.rule(&format!("(deny file-read* file-write* {filter})"));
        }
        for directory in directories {
            let filter = self.path("subpath", directory);
            self.rule(&format!("(deny file-read* file-write* {filter})"));
            let filter = self.path("subpath", directory);
            self.rule(&format!(
                "(deny network-outbound (remote unix-socket {filter}))"
            ));
        }
        let above: std::collections::BTreeSet<&Path> = (files.iter().chain(directories))
            .flat_map(|store| store.ancestors().skip(1))
            .filter(|directory| directory.parent().is_some())
            .collect();
        for directory in above {
            self.anchor(directory);
        }
    }

    /// Keeps the directory `path` from being renamed or removed.
    fn anchor(&mut self, path: &Path) {
        let filter = self.path("literal", path);
        self.rule(&format!(
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) {filter}))"
        ));
    }

    /// `sandbox-exec` running this profile, up to the program to run, in `root`.
    fn command(self, root: &Path) -> Command {
        let mut process = Command::new(PROGRAM);
        process.arg("-p").arg(&self.text);
        for (index, path) in self.params.iter().enumerate() {
            let mut definition = OsString::from(format!("-DP{index}="));
            definition.push(path);
            process.arg(definition);
        }
        process.arg("--").current_dir(root);
        process
    }
}

/// What a closed profile allows beyond reading: the real directories `writable` but the paths
/// `protected` in them (also those not made yet), with the directories `anchored` kept from
/// being renamed; the network if `network`. `hidden` stays hidden in all of them.
pub(super) struct Closed<'a> {
    pub writable: Vec<PathBuf>,
    pub protected: Vec<PathBuf>,
    pub anchored: Vec<PathBuf>,
    pub network: bool,
    pub hidden: (Vec<&'a PathBuf>, Vec<&'a PathBuf>),
}

/// `sandbox-exec` with the closed profile `closed`, up to the program to run, in `root`.
pub(super) fn closed(root: &Path, closed: Closed) -> Command {
    let mut profile = Profile::default();
    profile.text.push_str(BASE);
    for writable in &closed.writable {
        let mut parts = vec![profile.path("subpath", writable)];
        for kept in closed
            .protected
            .iter()
            .filter(|kept| kept.starts_with(writable))
        {
            // `subpath` alone would still let the protected directory itself be made.
            parts.push(format!("(require-not {})", profile.path("literal", kept)));
            parts.push(format!("(require-not {})", profile.path("subpath", kept)));
        }
        profile.rule(&format!(
            "(allow file-write* (require-all {}))",
            parts.join(" ")
        ));
    }
    if closed.network {
        profile.text.push_str(NETWORK);
    }
    profile.hide(&closed.hidden.0, &closed.hidden.1);
    for directory in &closed.anchored {
        profile.anchor(directory);
    }
    profile.text.push_str(CLOSED);
    profile.command(root)
}

/// `sandbox-exec` letting everything through but `hidden`, up to the program to run, in `root`.
pub(super) fn open(root: &Path, hidden: (Vec<&PathBuf>, Vec<&PathBuf>)) -> Command {
    let mut profile = Profile::default();
    profile.rule("(version 1)\n(allow default)");
    profile.hide(&hidden.0, &hidden.1);
    profile.command(root)
}

/// The directories a closed profile's commands write their temporary files in, by their real
/// paths: `/tmp`, `/var/tmp` and the user's `TMPDIR`, shared with everything else, as Codex
/// leaves them writable, where bubblewrap gives each command a `/tmp` of its own.
pub(super) fn temporary() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = ["/tmp".into(), "/var/tmp".into(), std::env::temp_dir()]
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .collect();
    found.sort();
    found.dedup();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(command: &Command) -> Vec<String> {
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned());
        args.collect()
    }

    /// The profile's text with each parameter replaced by its path, to read in assertions.
    fn spelled(command: &Command) -> String {
        let args = args(command);
        assert_eq!(command.get_program(), PROGRAM);
        assert_eq!(
            (args[0].as_str(), args.last().unwrap().as_str()),
            ("-p", "--")
        );
        let mut text = args[1].clone();
        let params = &args[2..args.len() - 1];
        for (index, param) in params.iter().enumerate().rev() {
            let path = param.strip_prefix(&format!("-DP{index}=")).unwrap();
            text = text.replace(&format!("(param \"P{index}\")"), &format!("\"{path}\""));
        }
        let depth = text.chars().try_fold(0i32, |depth, c| {
            let depth = depth + (c == '(') as i32 - (c == ')') as i32;
            (depth >= 0).then_some(depth)
        });
        assert_eq!(depth, Some(0), "parentheses balance:\n{text}");
        text
    }

    #[test]
    fn a_read_only_profile_writes_only_temporary_files_and_has_no_network() {
        let (file, directory) = (
            PathBuf::from("/Users/u/.netrc"),
            PathBuf::from("/Users/u/.ssh"),
        );
        let process = closed(
            Path::new("/w"),
            Closed {
                writable: vec!["/private/tmp".into()],
                protected: Vec::new(),
                anchored: Vec::new(),
                network: false,
                hidden: (vec![&file], vec![&directory]),
            },
        );
        assert_eq!(process.get_current_dir(), Some(Path::new("/w")));
        let text = spelled(&process);
        assert!(text.starts_with("(version 1)\n(deny default)\n"), "{text}");
        assert!(text.contains("(allow file-read*)\n"));
        assert!(text.contains("(allow file-write* (require-all (subpath \"/private/tmp\")))\n"));
        assert!(!text.contains("network-outbound)") && !text.contains("unix-socket))\n(allow"));
        let hidden = [
            "(deny file-read* file-write* (literal \"/Users/u/.netrc\"))",
            "(deny file-read* file-write* (subpath \"/Users/u/.ssh\"))",
            "(deny network-outbound (remote unix-socket (subpath \"/Users/u/.ssh\")))",
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal \"/Users/u\")))",
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) (literal \"/Users\")))",
        ];
        let allowed = text.rfind("(allow").unwrap();
        for rule in hidden {
            let at = text
                .find(rule)
                .unwrap_or_else(|| panic!("{rule} in\n{text}"));
            assert!(at > allowed, "{rule} after every allowance");
        }
        assert!(!text.contains("(literal \"/\")"));
        assert!(text.ends_with(CLOSED));
    }

    #[test]
    fn a_workspace_profile_writes_the_project_but_what_it_protects_and_shares_the_network() {
        let root = PathBuf::from("/Users/u/my \"project\"");
        let process = closed(
            &root,
            Closed {
                writable: vec![root.clone(), "/private/tmp".into()],
                protected: vec![root.join(".git"), root.join(".agents")],
                anchored: vec![root.clone()],
                network: true,
                hidden: (Vec::new(), Vec::new()),
            },
        );
        let text = spelled(&process);
        let project = "(allow file-write* (require-all (subpath \"/Users/u/my \"project\"\") \
            (require-not (literal \"/Users/u/my \"project\"/.git\")) \
            (require-not (subpath \"/Users/u/my \"project\"/.git\")) \
            (require-not (literal \"/Users/u/my \"project\"/.agents\")) \
            (require-not (subpath \"/Users/u/my \"project\"/.agents\"))))";
        assert!(text.contains(project), "{text}");
        assert!(text.contains("(allow file-write* (require-all (subpath \"/private/tmp\")))"));
        assert!(text.contains("(allow network-outbound network-inbound network-bind)"));
        assert!(text.contains(
            "(deny file-write-unlink (require-all (vnode-type DIRECTORY) \
             (literal \"/Users/u/my \"project\"\")))"
        ));
        // The path reaches sandbox-exec only as a parameter, whatever it holds.
        assert!(!args(&process)[1].contains("project"));
    }

    #[test]
    fn a_project_under_a_temporary_directory_stays_read_only_and_cannot_be_moved() {
        let tmp = std::fs::canonicalize("/tmp").unwrap();
        let scratch = tmp.join(format!("seatbelt-test-{}", std::process::id()));
        let root = scratch.join("clone");
        std::fs::create_dir_all(&root).unwrap();
        let hidden = super::super::Hidden::default();
        let text = spelled(&super::super::closed(
            &root,
            &hidden,
            false,
            false,
            &[],
            &[],
            None,
        ));
        std::fs::remove_dir_all(&scratch).unwrap();
        let (tmp, root) = (tmp.display(), root.display());
        assert!(
            text.contains(&format!(
                "(subpath \"{tmp}\") (require-not (literal \"{root}\")) \
             (require-not (subpath \"{root}\")))"
            )),
            "{text}"
        );
        for directory in [root.to_string(), scratch.display().to_string()] {
            let anchor = format!("(vnode-type DIRECTORY) (literal \"{directory}\"))");
            assert!(text.contains(&anchor), "{anchor} in\n{text}");
        }
        assert!(!text.contains("(literal \"/\")"));
    }

    #[test]
    fn granted_directories_are_writable_but_their_protected_paths() {
        let (build, other) = (
            PathBuf::from("/Users/u/p/build"),
            PathBuf::from("/Users/u/o"),
        );
        let hidden = super::super::Hidden::default();
        let granted = [build.clone(), other.clone(), other.join(".agents")];
        let kept = [other.join(".git")];
        let profile = super::super::closed(
            Path::new("/Users/u/p"),
            &hidden,
            false,
            false,
            &granted,
            &kept,
            None,
        );
        let text = spelled(&profile);
        let not = |path: &str| {
            format!("(require-not (literal \"{path}\")) (require-not (subpath \"{path}\"))")
        };
        let build = format!(
            "(allow file-write* (require-all (subpath \"/Users/u/p/build\") {} {} {}))",
            not("/Users/u/p/build/.agents"),
            not("/Users/u/p/build/.git"),
            not("/Users/u/p/build/.scramjet"),
        );
        assert!(text.contains(&build), "{build} in\n{text}");
        // `.agents` granted by name.
        let other = text
            .lines()
            .find(|line| {
                line.starts_with("(allow file-write* (require-all (subpath \"/Users/u/o\")")
            })
            .unwrap();
        assert!(other.contains(&not("/Users/u/o/.git")), "{other}");
        assert!(other.contains(&not("/Users/u/o/.scramjet")), "{other}");
        assert!(!other.contains(&not("/Users/u/o/.agents")), "{other}");
        assert!(text.contains("(allow file-write* (require-all (subpath \"/Users/u/o/.agents\") "));
        assert!(!text.contains("(subpath \"/Users/u/p\")"), "{text}");
        let anchor = "(vnode-type DIRECTORY) (literal \"/Users/u/o\"))";
        assert!(text.contains(anchor), "{text}");
    }

    #[test]
    fn an_open_profile_only_hides_the_stores() {
        let tmux = PathBuf::from("/private/tmp/tmux-501");
        let text = spelled(&open(Path::new("/w"), (Vec::new(), vec![&tmux])));
        assert!(text.starts_with("(version 1)\n(allow default)\n"), "{text}");
        assert!(text.contains("(deny file-read* file-write* (subpath \"/private/tmp/tmux-501\"))"));
        assert!(text.contains(
            "(deny network-outbound (remote unix-socket (subpath \"/private/tmp/tmux-501\")))"
        ));
        assert!(!text.contains("deny default") && !text.contains("xpc"));
    }
}
