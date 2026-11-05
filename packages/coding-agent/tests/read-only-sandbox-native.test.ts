import { spawnSync } from "node:child_process";
import { mkdtemp, readdir, readFile, realpath, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { release, tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { probeReadOnlySandbox, readOnlyArgvPrefix } from "../src/core/read-only-sandbox.js";
import { createLocalBashOperations } from "../src/core/tools/bash.js";

function quote(value: string): string {
	return `'${value.replaceAll("'", `'\\''`)}'`;
}

async function run(command: string, cwd: string, env?: NodeJS.ProcessEnv) {
	let output = "";
	const result = await createLocalBashOperations({ argvPrefix: readOnlyArgvPrefix() }).exec(command, cwd, {
		onData: (data) => {
			output += data.toString();
		},
		env,
		timeout: 10,
	});
	return { ...result, output };
}

function processes() {
	const result = spawnSync("ps", ["-axww", "-o", "pid=,pgid=,stat=,command="], { encoding: "utf8", timeout: 2000 });
	if (result.error || result.status !== 0) throw result.error ?? new Error(result.stderr);
	return result.stdout.split("\n").flatMap((line) => {
		const match = line.match(/^\s*(\d+)\s+(\d+)\s+(\S+)\s+(.*)$/);
		return match ? [{ pid: Number(match[1]), pgid: Number(match[2]), state: match[3], command: match[4] }] : [];
	});
}

async function waitUntil(predicate: () => boolean, timeout: number): Promise<boolean> {
	const deadline = Date.now() + timeout;
	do {
		if (predicate()) return true;
		await delay(50);
	} while (Date.now() < deadline);
	return false;
}

const DESCENDANT_FIXTURE = `
const { spawn } = require("node:child_process");
const http = require("node:http");
const [role, url] = process.argv.slice(2);
if (role === "parent") {
  const child = spawn(process.execPath, [__filename, "grandchild", url], {
    detached: true,
    stdio: "ignore",
  });
  child.unref();
} else {
  http.get(url, (response) => response.resume()).on("error", (error) => {
    console.error(error);
    process.exit(1);
  });
}
setInterval(() => {}, 1000);
`;

async function qualifyCleanup(cwd: string, kind: "abort" | "timeout") {
	const fixture = join(await realpath(cwd), "detached-descendant.cjs");
	await writeFile(fixture, DESCENDANT_FIXTURE);
	let ready = false;
	const server = createServer((_request, response) => {
		ready = true;
		response.end("ready");
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	const address = server.address();
	if (!address || typeof address === "string") throw new Error("Missing loopback address");
	const url = `http://127.0.0.1:${address.port}/${kind}`;
	const grandchildCommand = `${fixture} grandchild ${url}`;
	const controller = new AbortController();
	let output = "";
	let settled = false;
	const operation = createLocalBashOperations({ argvPrefix: readOnlyArgvPrefix() })
		.exec(`${quote(process.execPath)} ${quote(fixture)} parent ${quote(url)}`, cwd, {
			onData: (data) => {
				output += data.toString();
			},
			signal: controller.signal,
			timeout: kind === "timeout" ? 3 : 10,
		})
		.then(
			(result) => ({ result, error: undefined }),
			(error: Error) => ({ result: undefined, error }),
		)
		.finally(() => {
			settled = true;
		});
	let grandchildPid: number | undefined;
	try {
		expect(await waitUntil(() => ready || settled, 2500), output).toBe(true);
		expect(ready, `Grandchild never became ready: ${output}`).toBe(true);
		const descendants = processes().filter((entry) => entry.command.includes(grandchildCommand));
		expect(descendants, `Host ps must identify exactly one detached grandchild: ${grandchildCommand}`).toHaveLength(
			1,
		);
		const grandchild = descendants[0];
		grandchildPid = grandchild!.pid;
		expect(grandchild!.pgid).toBe(grandchildPid);
		expect(grandchild!.state.startsWith("Z")).toBe(false);
		if (kind === "abort") controller.abort();
		const outcome = await operation;
		expect(outcome.error?.message, output).toBe(kind === "abort" ? "aborted" : "timeout:3");
		const terminated = await waitUntil(() => {
			const entry = processes().find((candidate) => candidate.pid === grandchildPid);
			return !entry || entry.state.startsWith("Z");
		}, 2000);
		expect(terminated, `${process.platform} ${kind} left detached grandchild PID ${grandchildPid} alive`).toBe(true);
	} finally {
		controller.abort();
		await operation;
		for (const entry of processes()) {
			if (entry.pid === grandchildPid || entry.command.includes(`${fixture} `)) {
				try {
					process.kill(entry.pid, "SIGKILL");
				} catch (error) {
					expect((error as NodeJS.ErrnoException).code).toBe("ESRCH");
				}
			}
		}
		await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
	}
}

describe("read-only sandbox launch contract", () => {
	afterEach(() => vi.restoreAllMocks());

	it("rejects unsupported platforms", () => {
		vi.spyOn(process, "platform", "get").mockReturnValue("win32");
		expect(readOnlyArgvPrefix).toThrow("unsupported on platform win32");
		expect(probeReadOnlySandbox).toThrow("unsupported on platform win32");
	});

	it("rejects an empty prefix instead of running an unrestricted shell", () => {
		expect(() => createLocalBashOperations({ argvPrefix: [] })).toThrow("argvPrefix must contain an executable");
	});

	it("reports a missing backend actionably", () => {
		vi.spyOn(process, "platform", "get").mockReturnValue("linux");
		vi.stubEnv("PATH", "/scramjet-missing-sandbox-bin");
		try {
			expect(probeReadOnlySandbox).toThrow(/Cannot establish read-only.*\nInstall bubblewrap/s);
		} finally {
			vi.unstubAllEnvs();
		}
	});

	it.skipIf(process.platform === "win32")("reports a failed backend without a fallback", async () => {
		const bin = await mkdtemp(join(tmpdir(), "scramjet-failed-sandbox-"));
		await writeFile(join(bin, "bwrap"), "#!/bin/sh\nprintf 'namespace denied' >&2\nexit 1\n", { mode: 0o755 });
		vi.spyOn(process, "platform", "get").mockReturnValue("linux");
		vi.stubEnv("PATH", bin);
		try {
			expect(probeReadOnlySandbox).toThrow(/namespace denied.*\nInstall bubblewrap/s);
		} finally {
			vi.unstubAllEnvs();
			await rm(bin, { recursive: true, force: true });
		}
	});

	it.skipIf(process.platform === "win32")("does not run the command after a prefixed launch fails", async () => {
		const cwd = await mkdtemp(join(tmpdir(), "scramjet-failed-launch-"));
		try {
			await expect(
				createLocalBashOperations({ argvPrefix: ["/scramjet-missing-sandbox"] }).exec("touch escaped", cwd, {
					onData: () => {},
				}),
			).rejects.toMatchObject({ code: "ENOENT" });
			expect(await readdir(cwd)).toEqual([]);
		} finally {
			await rm(cwd, { recursive: true, force: true });
		}
	});

	it.skipIf(process.platform === "win32")("leaves unprefixed shell execution unchanged", async () => {
		let output = "";
		const result = await createLocalBashOperations().exec("printf unrestricted", process.cwd(), {
			onData: (data) => {
				output += data.toString();
			},
		});
		expect(result).toEqual({ exitCode: 0 });
		expect(output).toBe("unrestricted");
	});
});

describe.skipIf(process.env.SCRAMJET_NATIVE_SANDBOX_TESTS !== "1")(
	"native read-only sandbox qualification (requires SCRAMJET_NATIVE_SANDBOX_TESTS=1)",
	() => {
		let cwd: string;

		beforeAll(() => probeReadOnlySandbox());
		beforeEach(async () => {
			cwd = await mkdtemp(join(tmpdir(), "scramjet sandbox 'fixture-"));
		});
		afterEach(async () => {
			await rm(cwd, { recursive: true, force: true });
		});

		it("denies create, truncate, delete, rename and mkdir without changing host files", async () => {
			const original = "protected contents\n";
			for (const name of ["modify", "delete", "rename"]) await writeFile(join(cwd, name), original);
			const commands = [
				"touch created",
				"printf changed > modify",
				"rm delete",
				"mv rename replaced",
				"mkdir directory",
			];
			for (const command of commands) {
				const result = await run(command, cwd);
				expect(result.exitCode, `${command}: ${result.output}`).not.toBe(0);
			}
			expect((await readdir(cwd)).sort()).toEqual(["delete", "modify", "rename"]);
			for (const name of ["modify", "delete", "rename"])
				expect(await readFile(join(cwd, name), "utf8")).toBe(original);
		});

		it.skipIf(process.platform !== "linux" || !release().toLowerCase().includes("microsoft"))(
			"denies mutation under the WSL Windows mount /mnt/c",
			async () => {
				const windowsFixture = await mkdtemp("/mnt/c/scramjet-read-only-qualification-");
				try {
					await writeFile(join(windowsFixture, "original"), "protected");
					for (const command of ["touch created", "printf changed > original", "rm original"]) {
						const result = await run(command, windowsFixture);
						expect(result.exitCode, result.output).not.toBe(0);
					}
					expect(await readdir(windowsFixture)).toEqual(["original"]);
					expect(await readFile(join(windowsFixture, "original"), "utf8")).toBe("protected");
				} finally {
					await rm(windowsFixture, { recursive: true, force: true });
				}
			},
		);

		it("denies mutation from a backgrounded grandchild", async () => {
			const script = `const { spawn } = require("node:child_process");
const child = spawn(process.execPath, ["-e", "require('node:fs').writeFileSync('grandchild-created', 'changed')"], { stdio: "inherit" });
child.on("exit", (code) => process.exit(code ?? 1));`;
			const result = await run(`(${quote(process.execPath)} -e ${quote(script)}) & wait $!`, cwd);
			expect(result.exitCode, result.output).not.toBe(0);
			expect(await readdir(cwd)).toEqual([]);
		});

		it("preserves reads, cwd, /dev/null and the actual child environment", async () => {
			await writeFile(join(cwd, "readable"), "readable contents\n");
			const env = { ...process.env, GIT_OPTIONAL_LOCKS: "1", SCRAMJET_SANDBOX_ENV: "preserved" };
			const result = await run(
				`set -e; pwd -P; cat readable; printf discarded > /dev/null; printf '%s %s\\n' "$GIT_OPTIONAL_LOCKS" "$SCRAMJET_SANDBOX_ENV"`,
				cwd,
				env,
			);
			expect(result.exitCode, result.output).toBe(0);
			expect(result.output).toContain("readable contents\n0 preserved\n");
			expect(result.output.split("\n")[0]).toBe(await realpath(cwd));
			expect(env.GIT_OPTIONAL_LOCKS).toBe("1");
		});

		it("permits heredocs without writable host scratch", async () => {
			const result = await run("cat <<'EOF'\nheredoc works\nEOF", cwd);
			expect(result, result.output).toEqual({ exitCode: 0, output: "heredoc works\n" });
		});

		it.skipIf(process.platform !== "darwin")("permits reopening stdout and stderr through device paths", async () => {
			const result = await run("set -e; printf 'stdout\\n' > /dev/stdout; printf 'stderr\\n' > /dev/stderr", cwd);
			expect(result.exitCode, result.output).toBe(0);
			expect(result.output).toContain("stdout\n");
			expect(result.output).toContain("stderr\n");
		});

		it("leaves loopback HTTP access available", async () => {
			const server = createServer((_request, response) => response.end("network readable"));
			await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
			try {
				const address = server.address();
				if (!address || typeof address === "string") throw new Error("Missing loopback address");
				const script = `fetch('http://127.0.0.1:${address.port}').then(r => r.text()).then(text => console.log(text)).catch(() => process.exit(1))`;
				const result = await run(`${quote(process.execPath)} -e ${quote(script)}`, cwd);
				expect(result).toEqual({ exitCode: 0, output: "network readable\n" });
			} finally {
				await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
			}
		});

		it("does not leave a detached grandchild alive after abort", async () => qualifyCleanup(cwd, "abort"), 15000);
		it("does not leave a detached grandchild alive after timeout", async () => qualifyCleanup(cwd, "timeout"), 15000);
	},
);
