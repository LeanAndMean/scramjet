import { EventEmitter } from "node:events";
import { Writable } from "node:stream";
import { execSync, spawn } from "child_process";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RetainedViewport } from "../../tui/src/viewport.js";
import { copyToClipboard } from "../src/utils/clipboard.js";

vi.mock("child_process", () => ({ spawn: vi.fn(), execSync: vi.fn() }));
vi.mock("os", () => ({ platform: () => "linux" }));
vi.mock("../src/utils/clipboard-native.js", () => ({ clipboard: null }));
vi.mock("../src/utils/clipboard-image.js", () => ({ isWaylandSession: () => true }));

beforeEach(() => {
	vi.stubEnv("WAYLAND_DISPLAY", "synthetic");
	for (const key of ["DISPLAY", "TERMUX_VERSION", "SSH_CONNECTION", "SSH_CLIENT", "MOSH_CONNECTION"])
		vi.stubEnv(key, "");
	vi.mocked(execSync).mockReset().mockReturnValue(Buffer.alloc(0));
	vi.mocked(spawn).mockReset();
});
afterEach(() => {
	vi.restoreAllMocks();
	vi.unstubAllEnvs();
});

function backend() {
	const child = Object.assign(new EventEmitter(), {
		stdin: new Writable({
			write(_chunk, _encoding, done) {
				done();
			},
		}),
		kill: vi.fn(() => true),
		unref: vi.fn(),
	});
	vi.mocked(spawn).mockReturnValue(child as unknown as ReturnType<typeof spawn>);
	return child;
}

describe("clipboard backend settlement", () => {
	it("awaits Wayland stdin and successful parent exit with a bounded asynchronous spawn", async () => {
		const child = backend();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		let settled = false;
		const copy = copyToClipboard("synthetic").then(() => {
			settled = true;
		});
		await new Promise((resolve) => setImmediate(resolve));
		expect(settled).toBe(false);
		expect(spawn).toHaveBeenCalledWith("wl-copy", [], {
			stdio: ["pipe", "ignore", "ignore"],
			timeout: 5000,
			killSignal: "SIGKILL",
		});
		child.emit("exit", 0, null);
		await copy;
		expect(output).not.toHaveBeenCalled();
	});

	it("does not accept a zero exit before pending stdin fails", async () => {
		const child = backend();
		let finish!: (error: Error) => void;
		child.stdin._write = (_chunk, _encoding, done) => {
			finish = done;
		};
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		const copy = copyToClipboard("synthetic");
		child.emit("exit", 0, null);
		finish(new Error("write failed"));
		await copy;
		expect(output).toHaveBeenCalledExactlyOnceWith(`\x1b]52;c;${Buffer.from("synthetic").toString("base64")}\x07`);
	});

	it.each(["spawn", "stdin", "exit", "timeout"])(
		"falls back to OSC52 after observable %s failure",
		async (failure) => {
			const child = backend();
			const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
			const copy = copyToClipboard("synthetic");
			if (failure === "spawn") child.emit("error", new Error("ENOENT"));
			else if (failure === "stdin") child.stdin.emit("error", new Error("EPIPE"));
			else child.emit("exit", failure === "exit" ? 7 : null, failure === "timeout" ? "SIGKILL" : null);
			await copy;
			expect(output).toHaveBeenCalledExactlyOnceWith(`\x1b]52;c;${Buffer.from("synthetic").toString("base64")}\x07`);
		},
	);

	it("accepts X11 fallback without emitting OSC52", async () => {
		vi.stubEnv("DISPLAY", "synthetic");
		const child = backend();
		const output = vi.spyOn(process.stdout, "write").mockReturnValue(true);
		const copy = copyToClipboard("synthetic");
		child.emit("exit", 7, null);
		await copy;
		expect(execSync).toHaveBeenCalledWith(
			"xclip -selection clipboard",
			expect.objectContaining({ input: "synthetic" }),
		);
		expect(output).not.toHaveBeenCalled();
	});

	it.each([false, true])("retains selection only when all backends fail (fallback accepted=%s)", async (accepted) => {
		const child = backend();
		const output = vi.spyOn(process.stdout, "write").mockImplementation(() => {
			if (!accepted) throw new Error("terminal closed");
			return true;
		});
		const component = { render: () => ["synthetic"], invalidate() {} };
		const viewport = new RetainedViewport({ getBlocks: () => [{ component }], copy: copyToClipboard });
		viewport.update(30, 4);
		viewport.markPainted();
		for (const data of ["\x1b[<0;1;1M", "\x1b[<32;10;1M", "\x1b[<0;10;1m", "\x03"])
			viewport.handleInput(data, false, false);
		expect(viewport.notice).toBe("Copying selection…");
		child.emit("exit", 7, null);
		await vi.waitFor(() =>
			expect(viewport.notice).toBe(accepted ? undefined : "Copy failed: terminal closed; selection retained"),
		);
		expect(output).toHaveBeenCalledOnce();
	});
});
