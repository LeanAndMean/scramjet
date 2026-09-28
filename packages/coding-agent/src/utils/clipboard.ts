import { execFile, execSync, spawn } from "child_process";
import { platform } from "os";
import { waitForChildProcess } from "./child-process.js";
import { isWaylandSession } from "./clipboard-image.js";
import { clipboard } from "./clipboard-native.js";

type NativeClipboardExecOptions = {
	input: string;
	timeout: number;
	stdio: ["pipe", "ignore", "ignore"];
};

function copyToX11Clipboard(options: NativeClipboardExecOptions): void {
	try {
		execSync("xclip -selection clipboard", options);
	} catch {
		execSync("xsel --clipboard --input", options);
	}
}

const MAX_OSC52_ENCODED_LENGTH = 100_000;

function isRemoteSession(env: NodeJS.ProcessEnv = process.env): boolean {
	return Boolean(env.SSH_CONNECTION || env.SSH_CLIENT || env.MOSH_CONNECTION);
}

function emitOsc52(text: string): boolean {
	const encoded = Buffer.from(text).toString("base64");
	if (encoded.length > MAX_OSC52_ENCODED_LENGTH) {
		return false;
	}
	process.stdout.write(`\x1b]52;c;${encoded}\x07`);
	return true;
}

// SCRAMJET-DIVERGENCE: mouse reporting replaces the terminal's editor right-click paste.
export async function readClipboardText(): Promise<string> {
	if (isRemoteSession()) throw new Error("Use your terminal's Paste command in remote sessions");
	const p = platform();
	const wsl = p === "linux" && Boolean(process.env.WSL_DISTRO_NAME || process.env.WSL_INTEROP);
	if (clipboard && !wsl && p !== "linux") {
		try {
			return await clipboard.getText();
		} catch {
			// The platform command remains available if the optional addon fails.
		}
	}
	const commands: [string, string[]][] =
		wsl || p === "win32"
			? [
					[
						"powershell.exe",
						[
							"-NoProfile",
							"-NonInteractive",
							"-STA",
							"-Command",
							"Add-Type -AssemblyName System.Windows.Forms; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::Write([System.Windows.Forms.Clipboard]::GetText())",
						],
					],
				]
			: p === "darwin"
				? [["pbpaste", []]]
				: process.env.TERMUX_VERSION
					? [["termux-clipboard-get", []]]
					: [
							...(process.env.WAYLAND_DISPLAY
								? [["wl-paste", ["--no-newline", "--type", "text"]] as [string, string[]]]
								: []),
							...(process.env.DISPLAY
								? ([
										["xclip", ["-selection", "clipboard", "-o"]],
										["xsel", ["--clipboard", "--output"]],
									] as [string, string[]][])
								: []),
						];
	for (const [command, args] of commands) {
		try {
			return await new Promise<string>((resolve, reject) => {
				execFile(
					command,
					args,
					{ encoding: "utf8", timeout: 5000, maxBuffer: 10 * 1024 * 1024, killSignal: "SIGKILL" },
					(error, stdout) => {
						if (error) reject(error);
						else resolve(stdout);
					},
				);
			});
		} catch {
			// Try the next local backend without requesting remote clipboard access.
		}
	}
	throw new Error("Cannot read clipboard text; use your terminal's Paste command");
}

// SCRAMJET-DIVERGENCE: prewarm WSL interop without polling or caching clipboard contents.
export function createWslClipboardReader() {
	if (platform() !== "linux" || !(process.env.WSL_DISTRO_NAME || process.env.WSL_INTEROP) || isRemoteSession())
		return undefined;
	const maxBytes = 10 * 1024 * 1024;
	const maxWireBytes = 4 * Math.ceil(maxBytes / 3) + 6;
	const script = `
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ScramjetClipboard {
    [DllImport("user32.dll")] static extern bool OpenClipboard(IntPtr owner);
    [DllImport("user32.dll")] static extern bool CloseClipboard();
    [DllImport("user32.dll")] static extern bool IsClipboardFormatAvailable(uint format);
    [DllImport("user32.dll")] static extern IntPtr GetClipboardData(uint format);
    [DllImport("kernel32.dll")] static extern IntPtr GlobalLock(IntPtr handle);
    [DllImport("kernel32.dll")] static extern bool GlobalUnlock(IntPtr handle);
    [DllImport("kernel32.dll")] static extern UIntPtr GlobalSize(IntPtr handle);
    public static string ReadText() {
        for (int attempt = 0; !OpenClipboard(IntPtr.Zero); attempt++) {
            if (attempt == 9) throw new InvalidOperationException("Clipboard unavailable");
            System.Threading.Thread.Sleep(5);
        }
        try {
            if (!IsClipboardFormatAvailable(13)) return "";
            IntPtr handle = GetClipboardData(13);
            if (handle == IntPtr.Zero) throw new InvalidOperationException("Clipboard data unavailable");
            ulong size = GlobalSize(handle).ToUInt64();
            if (size < 2 || size > ${maxBytes * 2 + 2} || size % 2 != 0) throw new InvalidOperationException("Invalid clipboard size");
            IntPtr data = GlobalLock(handle);
            if (data == IntPtr.Zero) throw new InvalidOperationException("Clipboard data unavailable");
            try {
                string value = Marshal.PtrToStringUni(data, (int)(size / 2));
                int end = value.IndexOf('\\0');
                if (end < 0) throw new InvalidOperationException("Unterminated clipboard text");
                return value.Substring(0, end);
            } finally { GlobalUnlock(handle); }
        } finally { CloseClipboard(); }
    }
}
'@
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::Out.WriteLine('READY'); [Console]::Out.Flush()
while ($null -ne ($request = [Console]::In.ReadLine())) {
    if ($request -ne 'read') { break }
    try {
        $text = [ScramjetClipboard]::ReadText()
        if ([System.Text.Encoding]::UTF8.GetByteCount($text) -gt ${maxBytes}) {
            [Console]::Out.WriteLine('ERR:size')
        } else {
            $bytes = [System.Text.Encoding]::UTF8.GetBytes($text)
            [Console]::Out.WriteLine('OK:' + [Convert]::ToBase64String($bytes))
        }
    } catch { [Console]::Out.WriteLine('ERR:read') }
    finally { $text = $null; $bytes = $null }
    [Console]::Out.Flush()
}`;
	let worker: ReturnType<typeof launch> | undefined;
	function launch() {
		const child = spawn(
			"powershell.exe",
			[
				"-NoLogo",
				"-NoProfile",
				"-NonInteractive",
				"-STA",
				"-EncodedCommand",
				Buffer.from(script, "utf16le").toString("base64"),
			],
			{ stdio: ["pipe", "pipe", "ignore"] },
		);
		let resolveReady!: () => void;
		let rejectReady!: (error: Error) => void;
		const ready = new Promise<void>((resolve, reject) => {
			resolveReady = resolve;
			rejectReady = reject;
		});
		let initialized = false;
		let closed = false;
		let buffer = "";
		let pending: { resolve(text: string): void; reject(error: Error): void; promise: Promise<string> } | undefined;
		const kill = () => {
			child.kill("SIGKILL");
		};
		const fail = (error = new Error("Clipboard reader stopped; use your terminal's Paste command")) => {
			if (closed) return;
			closed = true;
			clearTimeout(timer);
			buffer = "";
			rejectReady(error);
			pending?.reject(error);
			pending = undefined;
			if (worker === instance) worker = undefined;
			kill();
		};
		let timer = setTimeout(
			() => fail(new Error("Clipboard reader startup timed out; use your terminal's Paste command")),
			5000,
		);
		const instance = {
			ready,
			close: fail,
			read(): Promise<string> {
				if (isRemoteSession())
					return Promise.reject(new Error("Use your terminal's Paste command in remote sessions"));
				if (closed || !initialized) return Promise.reject(new Error("Clipboard reader is not ready"));
				if (pending) return pending.promise;
				let resolve!: (text: string) => void;
				let reject!: (error: Error) => void;
				const promise = new Promise<string>((done, error) => {
					resolve = done;
					reject = error;
				});
				pending = { resolve, reject, promise };
				timer = setTimeout(
					() => fail(new Error("Clipboard read timed out; use your terminal's Paste command")),
					5000,
				);
				child.stdin.write("read\n", (error) => {
					if (error) fail(new Error("Clipboard reader input failed"));
				});
				return promise;
			},
		};
		process.once("exit", kill);
		child.once("spawn", () => {
			if (closed) kill();
		});
		void waitForChildProcess(child)
			.then(
				() => fail(),
				() => fail(new Error("Cannot start clipboard reader; use your terminal's Paste command")),
			)
			.finally(() => process.off("exit", kill));
		child.stdin.on("error", () => fail(new Error("Clipboard reader input failed")));
		child.stdout.on("error", () => fail(new Error("Clipboard reader output failed")));
		child.stdout.on("data", (chunk: Buffer) => {
			if (closed) return;
			if (buffer.length + chunk.length > (initialized ? maxWireBytes : 7)) {
				fail(new Error("Clipboard response exceeded its size limit"));
				return;
			}
			buffer += chunk.toString("utf8");
			if (!buffer.includes("\n")) return;
			const response = buffer;
			buffer = "";
			if (!initialized && /^READY\r?\n$/.test(response)) {
				initialized = true;
				clearTimeout(timer);
				resolveReady();
				return;
			}
			const match = /^OK:([A-Za-z0-9+/]*={0,2})\r?\n$/.exec(response);
			if (!pending || !match || match[1].length % 4 !== 0) {
				fail(new Error("Cannot read clipboard text; use your terminal's Paste command"));
				return;
			}
			const bytes = Buffer.from(match[1], "base64");
			if (bytes.length > maxBytes || bytes.toString("base64") !== match[1]) {
				fail(new Error("Invalid or oversized clipboard response"));
				return;
			}
			clearTimeout(timer);
			const request = pending;
			pending = undefined;
			request.resolve(bytes.toString("utf8"));
		});
		return instance;
	}
	return {
		start(): Promise<void> {
			if (isRemoteSession())
				return Promise.reject(new Error("Use your terminal's Paste command in remote sessions"));
			try {
				worker ??= launch();
				return worker.ready;
			} catch {
				return Promise.reject(new Error("Cannot start clipboard reader; use your terminal's Paste command"));
			}
		},
		read(): Promise<string> {
			return worker ? worker.read() : Promise.reject(new Error("Clipboard reader stopped; try pasting again"));
		},
		close(): void {
			worker?.close();
		},
	};
}

export async function copyToClipboard(text: string): Promise<void> {
	let copied = false;

	const p = platform();

	// Prefer direct clipboard writes. Emitting OSC 52 first can make terminals
	// write the same native clipboard concurrently with the addon, and very large
	// OSC 52 payloads can desynchronize terminal rendering.
	//
	// On Linux, skip the native addon. The underlying `clipboard-rs` crate is
	// X11-only and does not retain selection ownership after `set_text`
	// resolves, so on Wayland-only compositors (Hyprland, Niri, ...) and even
	// some X11 sessions the call resolves successfully without populating the
	// clipboard. The platform tools below (wl-copy, xclip, xsel) properly
	// daemonize and keep ownership.
	try {
		if (clipboard && p !== "linux") {
			await clipboard.setText(text);
			copied = true;
		}
	} catch {
		// Fall through to platform-specific clipboard tools.
	}

	const remote = isRemoteSession();
	if (copied && !remote) {
		return;
	}

	const options: NativeClipboardExecOptions = { input: text, timeout: 5000, stdio: ["pipe", "ignore", "ignore"] };

	if (!copied) {
		try {
			if (p === "darwin") {
				execSync("pbcopy", options);
				copied = true;
			} else if (p === "win32") {
				execSync("clip", options);
				copied = true;
			} else {
				// Linux. Try Termux, Wayland, or X11 clipboard tools.
				if (process.env.TERMUX_VERSION) {
					try {
						execSync("termux-clipboard-set", options);
						copied = true;
					} catch {
						// Fall back to Wayland or X11 tools.
					}
				}

				if (!copied) {
					const hasWaylandDisplay = Boolean(process.env.WAYLAND_DISPLAY);
					const hasX11Display = Boolean(process.env.DISPLAY);
					const isWayland = isWaylandSession();
					if (isWayland && hasWaylandDisplay) {
						try {
							// SCRAMJET-DIVERGENCE: await backend acceptance without execSync's daemon/fork hang.
							await new Promise<void>((resolve, reject) => {
								const proc = spawn("wl-copy", [], {
									stdio: ["pipe", "ignore", "ignore"],
									timeout: options.timeout,
									killSignal: "SIGKILL",
								});
								let inputFinished = false;
								let exited = false;
								proc.on("error", reject);
								proc.stdin.on("error", (error) => {
									proc.kill("SIGKILL");
									reject(error);
								});
								proc.once("exit", (code) => {
									if (code !== 0) {
										reject(new Error("wl-copy did not accept clipboard content"));
										return;
									}
									exited = true;
									if (inputFinished) resolve();
								});
								proc.stdin.end(text, (error?: Error | null) => {
									if (error) {
										reject(error);
										return;
									}
									inputFinished = true;
									if (exited) resolve();
								});
							});
							copied = true;
						} catch {
							if (hasX11Display) {
								copyToX11Clipboard(options);
								copied = true;
							}
						}
					} else if (hasX11Display) {
						copyToX11Clipboard(options);
						copied = true;
					}
				}
			}
		} catch {
			// Fall through to OSC 52 fallback.
		}
	}

	if (remote || !copied) {
		const osc52Copied = emitOsc52(text);
		copied = copied || osc52Copied;
	}

	if (!copied) {
		throw new Error("Failed to copy to clipboard");
	}
}
