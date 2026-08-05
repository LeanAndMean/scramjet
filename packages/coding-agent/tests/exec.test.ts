import { performance } from "node:perf_hooks";
import { describe, expect, it } from "vitest";
import { execCommand } from "../src/core/exec.js";

describe("execCommand", () => {
	it("writes exact UTF-8 bytes to stdin", async () => {
		const stdin = "first\r\nsecond\0雪💥without-final-newline";
		const result = await execCommand(
			process.execPath,
			[
				"-e",
				"const chunks = []; process.stdin.on('data', (chunk) => chunks.push(chunk)); process.stdin.on('end', () => process.stdout.write(Buffer.concat(chunks).toString('base64')));",
			],
			process.cwd(),
			{ stdin },
		);

		expect(result).toEqual({
			stdout: Buffer.from(stdin).toString("base64"),
			stderr: "",
			code: 0,
			killed: false,
		});
	});

	it("reports missing executables with supplied stdin as one structured spawn error", async () => {
		const result = await execCommand("scramjet-command-that-does-not-exist", [], process.cwd(), {
			stdin: "mutation body",
		});

		expect(result).toMatchObject({
			stdout: "",
			stderr: "",
			code: 1,
			killed: false,
			spawnError: { code: "ENOENT" },
		});
		expect(result.spawnError?.message).toContain("scramjet-command-that-does-not-exist");
		expect(result.stdinError).toBeUndefined();
	});

	it("reports stdin errors when the child closes the pipe", async () => {
		const result = await execCommand(
			process.execPath,
			["-e", "process.stdin.destroy(); setTimeout(() => process.exit(0), 100)"],
			process.cwd(),
			{ stdin: "x".repeat(8 * 1024 * 1024) },
		);

		expect(result.code).toBe(0);
		expect(result.stdinError?.code).toMatch(/^(EPIPE|ECONNRESET)$/);
		expect(result.stdinError?.message).toBeTruthy();
	});

	it("preserves immediate EOF and captured output when stdin is absent", async () => {
		const result = await execCommand(
			process.execPath,
			[
				"-e",
				"process.stdin.on('data', () => process.exit(9)); process.stdin.on('end', () => { process.stdout.write('out'); process.stderr.write('err'); process.exit(7); });",
			],
			process.cwd(),
		);

		expect(result).toEqual({ stdout: "out", stderr: "err", code: 7, killed: false });
	});

	it("settles after timing out with backpressured stdin", async () => {
		const startedAt = performance.now();
		const result = await execCommand(process.execPath, ["-e", "setInterval(() => {}, 1000)"], process.cwd(), {
			stdin: "x".repeat(8 * 1024 * 1024),
			timeout: 100,
		});

		expect(result.killed).toBe(true);
		expect(performance.now() - startedAt).toBeLessThan(2000);
	});

	it("settles after aborting with backpressured stdin", async () => {
		const controller = new AbortController();
		const startedAt = performance.now();
		const pending = execCommand(process.execPath, ["-e", "setInterval(() => {}, 1000)"], process.cwd(), {
			stdin: "x".repeat(8 * 1024 * 1024),
			signal: controller.signal,
		});
		setTimeout(() => controller.abort(), 100);

		const result = await pending;
		expect(result.killed).toBe(true);
		expect(performance.now() - startedAt).toBeLessThan(2000);
	});

	it.skipIf(process.platform === "win32")(
		"escalates to SIGKILL when a timed-out child ignores SIGTERM",
		async () => {
			const startedAt = performance.now();
			const result = await execCommand(
				process.execPath,
				["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"],
				process.cwd(),
				{ timeout: 100 },
			);

			expect(result.killed).toBe(true);
			expect(performance.now() - startedAt).toBeLessThan(6500);
		},
		7000,
	);
});
