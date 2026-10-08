import { spawnSync } from "node:child_process";

const MACOS_PROFILE = `(version 1)
(allow default)
(deny file-write*)
(allow file-write* (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))`;

// SCRAMJET-DIVERGENCE: Native read-only execution is opt-in and never falls back to an unrestricted shell.
export function readOnlyArgvPrefix(): string[] {
	switch (process.platform) {
		case "linux":
			return [
				"bwrap",
				"--ro-bind",
				"/",
				"/",
				"--dev",
				"/dev",
				"--proc",
				"/proc",
				"--unshare-pid",
				"--die-with-parent",
				"--new-session",
				"--setenv",
				"GIT_OPTIONAL_LOCKS",
				"0",
				"--",
			];
		case "darwin":
			return ["/usr/bin/sandbox-exec", "-p", MACOS_PROFILE, "/usr/bin/env", "GIT_OPTIONAL_LOCKS=0"];
		default:
			throw new Error(`Read-only filesystem access is unsupported on platform ${process.platform}.`);
	}
}

export function probeReadOnlySandbox(): void {
	const [executable, ...args] = readOnlyArgvPrefix();
	const result = spawnSync(executable, [...args, process.platform === "darwin" ? "/usr/bin/true" : "/bin/true"], {
		encoding: "utf8",
		timeout: 5000,
		maxBuffer: 64 * 1024,
		stdio: ["ignore", "pipe", "pipe"],
	});
	if (!result.error && result.status === 0) return;
	const guidance =
		process.platform === "linux"
			? "Install bubblewrap and ensure unprivileged user namespaces are permitted. On Ubuntu 24.04+, check kernel.apparmor_restrict_unprivileged_userns with your administrator."
			: "Ensure /usr/bin/sandbox-exec is available and allowed by your macOS security policy.";
	const reason =
		result.error?.message || result.stderr.trim() || `exit status ${result.status}, signal ${result.signal}`;
	throw new Error(`Cannot establish read-only filesystem access: ${reason}\n${guidance}`);
}
