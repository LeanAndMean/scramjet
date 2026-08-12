/**
 * Shared command execution utilities for extensions and custom tools.
 */

import { spawn } from "node:child_process";
import { StringDecoder } from "node:string_decoder";
import { waitForChildProcess } from "../utils/child-process.js";

/**
 * Options for executing shell commands.
 */
export interface ExecOptions {
	/** AbortSignal to cancel the command */
	signal?: AbortSignal;
	/** Timeout in milliseconds */
	timeout?: number;
	/** Working directory */
	cwd?: string;
	/** Exact UTF-8 input to write before closing stdin */
	stdin?: string;
}

export interface ExecError {
	message: string;
	code?: string;
}

/**
 * Result of executing a shell command.
 */
export interface ExecResult {
	stdout: string;
	stderr: string;
	code: number;
	killed: boolean;
	spawnError?: ExecError;
	stdinError?: ExecError;
}

/**
 * Execute a shell command and return stdout/stderr/code.
 * Supports timeout and abort signal.
 */
export async function execCommand(
	command: string,
	args: string[],
	cwd: string,
	options?: ExecOptions,
): Promise<ExecResult> {
	return new Promise((resolve) => {
		const proc = spawn(command, args, {
			cwd,
			shell: false,
			stdio: [options?.stdin === undefined ? "ignore" : "pipe", "pipe", "pipe"],
		});

		let stdout = "";
		let stderr = "";
		// SCRAMJET-DIVERGENCE: decode process output across chunk boundaries for exact publication verification (#479).
		const stdoutDecoder = new StringDecoder("utf8");
		const stderrDecoder = new StringDecoder("utf8");
		let killed = false;
		let settled = false;
		let spawned = false;
		let terminationRequested = false;
		let spawnError: ExecError | undefined;
		let stdinError: ExecError | undefined;
		let timeoutId: NodeJS.Timeout | undefined;
		let killTimeoutId: NodeJS.Timeout | undefined;

		// SCRAMJET-DIVERGENCE: timeout escalation tracks settlement rather than signal delivery (#432).
		const terminateSpawnedProcess = () => {
			if (killed || !spawned || !proc.kill("SIGTERM")) return;
			killed = true;
			killTimeoutId = setTimeout(() => {
				if (!settled) proc.kill("SIGKILL");
			}, 5000);
		};
		const killProcess = () => {
			terminationRequested = true;
			terminateSpawnedProcess();
		};

		// Handle abort signal
		if (options?.signal) {
			if (options.signal.aborted) {
				killProcess();
			} else {
				options.signal.addEventListener("abort", killProcess, { once: true });
			}
		}

		// Handle timeout
		if (options?.timeout && options.timeout > 0) {
			timeoutId = setTimeout(() => {
				killProcess();
			}, options.timeout);
		}

		proc.once("spawn", () => {
			spawned = true;
			if (terminationRequested) terminateSpawnedProcess();
		});

		proc.stdout?.on("data", (data) => {
			stdout += stdoutDecoder.write(data);
		});

		proc.stderr?.on("data", (data) => {
			stderr += stderrDecoder.write(data);
		});

		const errorDetails = (error: Error): ExecError => {
			const code = (error as NodeJS.ErrnoException).code;
			return code === undefined ? { message: error.message } : { message: error.message, code };
		};

		const stdinSettled = new Promise<void>((resolveStdin) => {
			if (options?.stdin === undefined || proc.stdin === null) {
				resolveStdin();
				return;
			}

			const onStdinError = (error: Error) => {
				stdinError ??= errorDetails(error);
			};
			proc.stdin.on("error", onStdinError);
			proc.stdin.once("close", () => {
				proc.stdin?.removeListener("error", onStdinError);
				resolveStdin();
			});
			proc.stdin.end(options.stdin, "utf8");
		});

		const processSettled = waitForChildProcess(proc)
			.then((code) => code ?? 0)
			.catch((error: Error) => {
				if (!spawned) spawnError = errorDetails(error);
				return 1;
			});

		Promise.all([processSettled, stdinSettled]).then(([code]) => {
			stdout += stdoutDecoder.end();
			stderr += stderrDecoder.end();
			settled = true;
			if (timeoutId) clearTimeout(timeoutId);
			if (killTimeoutId) clearTimeout(killTimeoutId);
			if (options?.signal) options.signal.removeEventListener("abort", killProcess);
			resolve({ stdout, stderr, code, killed, spawnError, stdinError });
		});
	});
}
