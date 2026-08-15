import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";

const { spawnMock } = vi.hoisted(() => ({ spawnMock: vi.fn() }));
vi.mock("node:child_process", () => ({ spawn: spawnMock }));

import { execCommand } from "../src/core/exec.js";

function child() {
	const process = new EventEmitter() as EventEmitter & {
		stdout: PassThrough;
		stderr: PassThrough;
		stdin: null;
		kill: ReturnType<typeof vi.fn>;
	};
	process.stdout = new PassThrough();
	process.stderr = new PassThrough();
	process.stdin = null;
	process.kill = vi.fn(() => {
		queueMicrotask(() => {
			process.stdout.end();
			process.stderr.end();
			process.emit("exit", 1);
			process.emit("close", 1);
		});
		return true;
	});
	return process;
}

describe("execCommand output stream errors", () => {
	it.each(["stdout", "stderr"] as const)("settles a spawned %s error without throwing", async (stream) => {
		const process = child();
		spawnMock.mockReturnValueOnce(process);
		const execution = execCommand("command", [], "/repo");
		process.emit("spawn");
		process[stream].emit("error", Object.assign(new Error(`${stream} failed`), { code: "EIO" }));
		const result = await execution;
		expect(result[`${stream}Error`]).toEqual({ message: `${stream} failed`, code: "EIO" });
		expect(process.kill).toHaveBeenCalledWith("SIGTERM");
	});
});
