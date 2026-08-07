/**
 * Shared command execution utilities for extensions and custom tools.
 */

import { spawn } from "node:child_process";
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
	/** Exact string content to write to stdin */
	stdin?: string;
}

/**
 * Result of executing a shell command.
 */
export interface ExecResult {
	stdout: string;
	stderr: string;
	code: number;
	killed: boolean;
	spawnError?: { message: string; code?: string };
	stdinError?: { message: string; code?: string };
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
		const stdin = options?.stdin;
		const proc = spawn(command, args, {
			cwd,
			shell: false,
			stdio: [stdin === undefined ? "ignore" : "pipe", "pipe", "pipe"],
		});

		let stdout = "";
		let stderr = "";
		let killed = false;
		let settled = false;
		let spawned = false;
		let spawnError: ExecResult["spawnError"];
		let stdinError: ExecResult["stdinError"];
		let timeoutId: NodeJS.Timeout | undefined;
		let killTimeoutId: NodeJS.Timeout | undefined;

		// SCRAMJET-DIVERGENCE: pi.exec supports exact stdin and structured process errors (#468).
		const errorDetails = (error: unknown) => ({
			message: error instanceof Error ? error.message : String(error),
			...(error instanceof Error && "code" in error && typeof error.code === "string" ? { code: error.code } : {}),
		});

		proc.once("spawn", () => {
			spawned = true;
		});
		proc.once("error", (error) => {
			if (!spawned) spawnError = errorDetails(error);
		});

		// SCRAMJET-DIVERGENCE: timeout escalation tracks settlement rather than signal delivery (#432).
		const killProcess = () => {
			if (!killed) {
				killed = true;
				proc.kill("SIGTERM");
				killTimeoutId = setTimeout(() => {
					if (!settled) proc.kill("SIGKILL");
				}, 5000);
			}
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

		proc.stdout?.on("data", (data) => {
			stdout += data.toString();
		});

		proc.stderr?.on("data", (data) => {
			stderr += data.toString();
		});

		const stdinDone = new Promise<void>((resolveStdin) => {
			if (stdin === undefined || proc.stdin === null) {
				resolveStdin();
				return;
			}

			let stdinSettled = false;
			const settleStdin = (error?: unknown) => {
				if (stdinSettled) return;
				stdinSettled = true;
				if (error != null) stdinError = errorDetails(error);
				resolveStdin();
			};

			proc.stdin.on("error", settleStdin);
			proc.once("error", () => settleStdin());
			proc.once("close", () => settleStdin());
			proc.once("spawn", () => proc.stdin?.end(stdin, settleStdin));
		});

		// Wait for process termination without hanging on inherited stdio handles
		// held open by detached descendants.
		const finish = (code: number) => {
			settled = true;
			if (timeoutId) clearTimeout(timeoutId);
			if (killTimeoutId) clearTimeout(killTimeoutId);
			if (options?.signal) options.signal.removeEventListener("abort", killProcess);
			resolve({
				stdout,
				stderr,
				code,
				killed,
				...(spawnError ? { spawnError } : {}),
				...(stdinError ? { stdinError } : {}),
			});
		};

		Promise.all([
			waitForChildProcess(proc)
				.then((code) => code ?? 0)
				.catch(() => 1),
			stdinDone,
		]).then(([code]) => finish(code));
	});
}
