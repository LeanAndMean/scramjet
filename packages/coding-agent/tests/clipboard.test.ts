import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { execFile, execSync, spawn } from "child_process";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { RetainedViewport } from "../../tui/src/viewport.js";
import { copyToClipboard, createWslClipboardReader, readClipboardText } from "../src/utils/clipboard.js";

vi.mock("child_process", () => ({ spawn: vi.fn(), execSync: vi.fn(), execFile: vi.fn() }));
vi.mock("os", () => ({ platform: () => "linux" }));
vi.mock("../src/utils/clipboard-native.js", () => ({ clipboard: null }));
vi.mock("../src/utils/clipboard-image.js", () => ({ isWaylandSession: () => true }));

beforeEach(() => {
	vi.stubEnv("WAYLAND_DISPLAY", "synthetic");
	for (const key of [
		"DISPLAY",
		"TERMUX_VERSION",
		"SSH_CONNECTION",
		"SSH_CLIENT",
		"MOSH_CONNECTION",
		"WSL_DISTRO_NAME",
		"WSL_INTEROP",
	])
		vi.stubEnv(key, "");
	vi.mocked(execSync).mockReset().mockReturnValue(Buffer.alloc(0));
	vi.mocked(spawn).mockReset();
	vi.mocked(execFile).mockReset();
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

describe("clipboard text reading", () => {
	function respond(text: string, error: Error | null = null) {
		vi.mocked(execFile).mockImplementationOnce(((
			_file: string,
			_args: string[],
			_options: unknown,
			callback: (error: Error | null, stdout: string) => void,
		) => {
			callback(error, text);
		}) as typeof execFile);
	}

	it("reads the Windows clipboard on WSL with exact Unicode and trailing newlines", async () => {
		vi.stubEnv("WSL_DISTRO_NAME", "Ubuntu");
		respond("café 界 é\r\n\r\n");
		expect(await readClipboardText()).toBe("café 界 é\r\n\r\n");
		expect(execFile).toHaveBeenCalledWith(
			"powershell.exe",
			expect.arrayContaining(["-NoProfile", "-STA", expect.stringContaining("UTF8Encoding")]),
			expect.objectContaining({ timeout: 5000, maxBuffer: 10 * 1024 * 1024 }),
			expect.any(Function),
		);
	});

	it("preserves empty text without falling through to another clipboard", async () => {
		respond("");
		expect(await readClipboardText()).toBe("");
		expect(execFile).toHaveBeenCalledExactlyOnceWith(
			"wl-paste",
			["--no-newline", "--type", "text"],
			expect.any(Object),
			expect.any(Function),
		);
	});

	it("falls back after an observable local backend failure", async () => {
		vi.stubEnv("DISPLAY", "synthetic");
		respond("", new Error("unavailable"));
		respond("X11");
		expect(await readClipboardText()).toBe("X11");
		expect(vi.mocked(execFile).mock.calls.map(([file]) => file)).toEqual(["wl-paste", "xclip"]);
	});

	it.each(["remote", "unavailable"])("reports %s clipboard access without a remote query", async (reason) => {
		if (reason === "remote") vi.stubEnv("SSH_CONNECTION", "synthetic");
		else vi.stubEnv("WAYLAND_DISPLAY", "");
		await expect(readClipboardText()).rejects.toThrow(/terminal's Paste/);
		expect(execFile).not.toHaveBeenCalled();
	});
});

describe("WSL clipboard reader lifetime", () => {
	const children: ReturnType<typeof worker>[] = [];
	const readers: NonNullable<ReturnType<typeof createWslClipboardReader>>[] = [];
	function worker() {
		const child = Object.assign(new EventEmitter(), {
			stdin: new PassThrough(),
			stdout: new PassThrough(),
			stderr: null,
			kill: vi.fn(() => true),
		});
		children.push(child);
		vi.mocked(spawn).mockReturnValueOnce(child as unknown as ReturnType<typeof spawn>);
		return child;
	}
	function reader() {
		vi.stubEnv("WSL_DISTRO_NAME", "Ubuntu");
		const result = createWslClipboardReader()!;
		readers.push(result);
		return result;
	}
	afterEach(async () => {
		for (const item of readers.splice(0)) item.close();
		for (const child of children.splice(0)) {
			child.stdout.end();
			child.emit("exit", 0, null);
			child.emit("close", 0, null);
		}
		vi.useRealTimers();
		await new Promise((resolve) => setImmediate(resolve));
	});
	it("prewarms without reading, then reads fresh Unicode/empty values through one process", async () => {
		const child = worker();
		const client = reader();
		const ready = client.start();
		expect(client.start()).toBe(ready);
		expect(child.stdin.read()).toBeNull();
		child.stdout.write("REA");
		child.stdout.write("DY\r\n");
		await ready;
		expect(child.stdin.read()).toBeNull();
		for (const text of ["café 界 é\r\n\r\n", "", "fresh"]) {
			const request = client.read();
			expect(client.read()).toBe(request);
			expect(child.stdin.read().toString()).toBe("read\n");
			const line = `OK:${Buffer.from(text).toString("base64")}\r\n`;
			child.stdout.write(line.slice(0, 4));
			child.stdout.write(line.slice(4));
			expect(await request).toBe(text);
		}
		expect(spawn).toHaveBeenCalledOnce();
		client.close();
		client.close();
		expect(child.kill).toHaveBeenCalledExactlyOnceWith("SIGKILL");
	});
	it.each(["startup", "read"])(
		"times out %s without retrying, and permits a later explicit restart",
		async (phase) => {
			vi.useFakeTimers();
			const child = worker();
			const client = reader();
			let request: Promise<unknown> = client.start();
			if (phase === "read") {
				child.stdout.write("READY\n");
				await request;
				request = client.read();
			}
			const rejected = expect(request).rejects.toThrow(/timed out/);
			await vi.advanceTimersByTimeAsync(5000);
			await rejected;
			expect(child.kill).toHaveBeenCalledOnce();
			expect(spawn).toHaveBeenCalledOnce();
			const fresh = worker();
			const ready = client.start();
			fresh.stdout.write("READY\n");
			await ready;
			expect(spawn).toHaveBeenCalledTimes(2);
		},
	);
	it.each(["ERR:read\n", "OK:!!!\n", "OK:Zg=\n", "OK:Zh==\n", "OK:Zg==\nOK:Zg==\n"])(
		"rejects malformed or failed responses %j without retry",
		async (response) => {
			const child = worker();
			const client = reader();
			const ready = client.start();
			child.stdout.write("READY\n");
			await ready;
			const request = client.read();
			const rejected = expect(request).rejects.toThrow();
			child.stdout.write(response);
			await rejected;
			expect(spawn).toHaveBeenCalledOnce();
			expect(child.kill).toHaveBeenCalledOnce();
		},
	);
	it("bounds unterminated response bytes before decoding", async () => {
		const child = worker();
		const client = reader();
		const ready = client.start();
		child.stdout.write("READY\n");
		await ready;
		const request = client.read();
		const rejected = expect(request).rejects.toThrow(/size limit/);
		child.stdout.write("A".repeat(14 * 1024 * 1024));
		await rejected;
	});
	it.each(["startup", "read"])(
		"cancels %s on close and isolates late output from a restarted worker",
		async (phase) => {
			const child = worker();
			const client = reader();
			let request: Promise<unknown> = client.start();
			if (phase === "read") {
				child.stdout.write("READY\n");
				await request;
				request = client.read();
			}
			const rejected = expect(request).rejects.toThrow(/stopped/);
			client.close();
			await rejected;
			const next = worker();
			const ready = client.start();
			child.stdout.write("READY\nOK:c3RhbGU=\n");
			next.stdout.write("READY\n");
			await ready;
			expect(next.stdin.read()).toBeNull();
		},
	);
	it.each(["error", "exit", "stdin", "stdout"])("rejects pending work after child %s", async (event) => {
		const child = worker();
		const client = reader();
		const ready = client.start();
		child.stdout.write("READY\n");
		await ready;
		const request = client.read();
		const rejected = expect(request).rejects.toThrow();
		if (event === "exit") {
			child.stdout.end();
			child.emit("exit", 1, null);
		} else if (event === "error") child.emit("error", new Error("synthetic"));
		else child[event as "stdin" | "stdout"].emit("error", new Error("synthetic"));
		await rejected;
	});
	it("never starts outside WSL or over remote connections", () => {
		expect(createWslClipboardReader()).toBeUndefined();
		vi.stubEnv("WSL_DISTRO_NAME", "Ubuntu");
		vi.stubEnv("SSH_CONNECTION", "synthetic");
		expect(createWslClipboardReader()).toBeUndefined();
		expect(spawn).not.toHaveBeenCalled();
	});
});

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
		for (const data of ["\x1b[<0;1;1M", "\x1b[<32;10;1M", "\x1b[<0;10;1m"]) viewport.handleInput(data, false, false);
		viewport.markPainted();
		viewport.handleInput("\x03", false, false);
		expect(viewport.notice).toBeUndefined();
		expect(viewport.state.height).toBe(4);
		child.emit("exit", 7, null);
		await vi.waitFor(() =>
			expect(viewport.notice).toBe(accepted ? undefined : "Copy failed: terminal closed; selection retained"),
		);
		expect(output).toHaveBeenCalledOnce();
		if (accepted) await vi.waitFor(() => expect(viewport.handleInput("\x03", false, false)).toBe(false));
	});
});
