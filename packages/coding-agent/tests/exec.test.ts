import { performance } from "node:perf_hooks";
import { describe, expect, it } from "vitest";
import { execCommand } from "../src/core/exec.js";

describe("execCommand", () => {
	it("writes stdin exactly without appending a newline", async () => {
		const stdin = "Unicode: café 中文\0\r\nno final newline";
		const result = await execCommand(process.execPath, ["-e", "process.stdin.pipe(process.stdout)"], process.cwd(), {
			stdin,
		});

		expect(result).toMatchObject({ stdout: stdin, code: 0, killed: false });
		expect(result.spawnError).toBeUndefined();
		expect(result.stdinError).toBeUndefined();
	});

	it.skipIf(process.platform === "win32")("decodes UTF-8 sequences split across stream chunks", async () => {
		const script = `
			const stdout = Buffer.from("café");
			const stderr = Buffer.from("東京");
			process.stdout.write(stdout.subarray(0, 4));
			process.stderr.write(stderr.subarray(0, 1));
			setTimeout(() => {
				process.stdout.write(stdout.subarray(4));
				process.stderr.write(stderr.subarray(1));
			}, 20);
		`;
		const result = await execCommand(process.execPath, ["-e", script], process.cwd());

		expect(result).toMatchObject({ stdout: "café", stderr: "東京", code: 0 });
		expect(result.stdout + result.stderr).not.toContain("�");
	});

	it("reports a pre-spawn failure", async () => {
		const result = await execCommand("scramjet-command-that-does-not-exist", [], process.cwd(), {
			stdin: "sensitive-marker",
		});

		expect(result.code).toBe(1);
		expect(result.spawnError).toMatchObject({ code: "ENOENT" });
		expect(JSON.stringify(result.spawnError)).not.toContain("sensitive-marker");
	});

	it.skipIf(process.platform === "win32")("reports stdin failure after spawn without including input", async () => {
		const stdin = "sensitive-marker".repeat(1024 * 1024);
		const result = await execCommand(
			process.execPath,
			["-e", "process.stdin.destroy(); setTimeout(() => process.exit(0), 50)"],
			process.cwd(),
			{ stdin },
		);

		expect(result.spawnError).toBeUndefined();
		expect(result.stdinError).toBeDefined();
		expect(JSON.stringify(result.stdinError)).not.toContain("sensitive-marker");
	});

	it.skipIf(process.platform === "win32")("honors an already-aborted signal after spawn", async () => {
		const controller = new AbortController();
		controller.abort();

		const result = await execCommand(process.execPath, ["-e", "setInterval(() => {}, 1000)"], process.cwd(), {
			signal: controller.signal,
		});

		expect(result.killed).toBe(true);
	});

	it.skipIf(process.platform === "win32")("settles pending stdin when aborted", async () => {
		const controller = new AbortController();
		setTimeout(() => controller.abort(), 50);

		const result = await execCommand(
			process.execPath,
			["-e", "process.stdin.pause(); setInterval(() => {}, 1000)"],
			process.cwd(),
			{ signal: controller.signal, stdin: "input".repeat(1024 * 1024) },
		);

		expect(result.killed).toBe(true);
	});

	it.skipIf(process.platform === "win32")(
		"escalates to SIGKILL when a timed-out child ignores SIGTERM",
		async () => {
			const startedAt = performance.now();
			const result = await execCommand(
				process.execPath,
				["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"],
				process.cwd(),
				{ timeout: 100, stdin: "timeout input" },
			);

			expect(result.killed).toBe(true);
			expect(result.stdinError).toBeUndefined();
			expect(performance.now() - startedAt).toBeLessThan(6500);
		},
		7000,
	);
});
