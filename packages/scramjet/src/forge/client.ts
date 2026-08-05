import { createHash } from "node:crypto";
import type { ExecOptions, ExecResult } from "@leanandmean/coding-agent";
import type { ForgeRepository } from "./types.js";

export const FORGE_EXEC_TIMEOUT_MS = 3000;

export type ForgeExec = (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;

export interface ForgeInvocation {
	command: string;
	args: string[];
	cwd: string;
	stdin?: string;
	signal?: AbortSignal;
	timeout?: number;
}

export type ForgeCommandErrorKind = "missing-executable" | "cancelled" | "timeout" | "stdin" | "failed" | "execution";

export interface ForgeInvocationDiagnostic {
	command: string;
	args: string[];
	cwd: string;
	stdin?: { bytes: number; sha256: string };
}

export class ForgeCommandError extends Error {
	readonly kind: ForgeCommandErrorKind;
	readonly invocation: ForgeInvocationDiagnostic;

	constructor(kind: ForgeCommandErrorKind, invocation: ForgeInvocationDiagnostic, cause?: unknown) {
		super(messageForFailure(kind, invocation), cause === undefined ? undefined : { cause });
		this.name = "ForgeCommandError";
		this.kind = kind;
		this.invocation = invocation;
	}
}

function messageForFailure(kind: ForgeCommandErrorKind, invocation: ForgeInvocationDiagnostic): string {
	const command = [invocation.command, ...invocation.args].map((part) => JSON.stringify(part)).join(" ");
	const stdin = invocation.stdin
		? ` with ${invocation.stdin.bytes} stdin bytes (sha256 ${invocation.stdin.sha256})`
		: "";
	const reason = {
		"missing-executable": "Executable is missing",
		cancelled: "Command was cancelled",
		timeout: "Command timed out",
		stdin: "Command could not receive stdin",
		failed: "Command failed",
		execution: "Command execution failed",
	}[kind];
	return `${reason}: ${command}${stdin}`;
}

function diagnosticFor(invocation: ForgeInvocation): ForgeInvocationDiagnostic {
	return {
		command: invocation.command,
		args: [...invocation.args],
		cwd: invocation.cwd,
		...(invocation.stdin === undefined
			? {}
			: {
					stdin: {
						bytes: Buffer.byteLength(invocation.stdin, "utf8"),
						sha256: createHash("sha256").update(invocation.stdin, "utf8").digest("hex"),
					},
				}),
	};
}

export async function runForgeCommand(exec: ForgeExec, invocation: ForgeInvocation): Promise<ExecResult> {
	const diagnostic = diagnosticFor(invocation);
	let result: ExecResult;
	try {
		result = await exec(invocation.command, invocation.args, {
			cwd: invocation.cwd,
			stdin: invocation.stdin,
			timeout: invocation.timeout,
			signal: invocation.signal,
		});
	} catch (error) {
		throw new ForgeCommandError("execution", diagnostic, error);
	}

	if (result.spawnError?.code === "ENOENT") throw new ForgeCommandError("missing-executable", diagnostic);
	if (result.killed && invocation.signal?.aborted) throw new ForgeCommandError("cancelled", diagnostic);
	if (result.killed) throw new ForgeCommandError("timeout", diagnostic);
	if (result.stdinError) throw new ForgeCommandError("stdin", diagnostic);
	if (result.spawnError || result.code !== 0) throw new ForgeCommandError("failed", diagnostic);
	return result;
}

function invalidRemote(): never {
	throw new Error("Origin is not a supported GitHub or GitLab origin");
}

function repositoryFor(host: string, rawPath: string): ForgeRepository {
	if (host !== "github.com" && host !== "gitlab.com") invalidRemote();
	if (rawPath.startsWith("/") || rawPath.endsWith("/") || rawPath.includes("//") || rawPath.includes("%")) {
		invalidRemote();
	}
	const segments = rawPath.split("/");
	const last = segments.at(-1);
	if (last?.endsWith(".git")) segments[segments.length - 1] = last.slice(0, -4);
	if (
		segments.some((segment) => !/^[A-Za-z0-9._-]+$/.test(segment) || segment === "." || segment === "..") ||
		(host === "github.com" ? segments.length !== 2 : segments.length < 2)
	) {
		invalidRemote();
	}
	return {
		forge: host === "github.com" ? "github" : "gitlab",
		host,
		projectPath: segments.join("/"),
	};
}

export function parseForgeRemote(remote: string): ForgeRepository {
	const value = remote.trim();
	const match =
		/^https:\/\/(github\.com|gitlab\.com)\/(.+)$/.exec(value) ??
		/^ssh:\/\/git@(github\.com|gitlab\.com)\/(.+)$/.exec(value) ??
		/^git@(github\.com|gitlab\.com):(.+)$/.exec(value);
	if (!match) return invalidRemote();
	return repositoryFor(match[1], match[2]);
}

export async function resolveCurrentRepository(
	exec: ForgeExec,
	cwd: string,
	signal?: AbortSignal,
): Promise<ForgeRepository> {
	const result = await runForgeCommand(exec, {
		command: "git",
		args: ["remote", "get-url", "origin"],
		cwd,
		signal,
		timeout: FORGE_EXEC_TIMEOUT_MS,
	});
	return parseForgeRemote(result.stdout);
}

const mutationQueues = new Map<string, Promise<void>>();

export async function withForgeMutationQueue<T>(key: string, operation: () => Promise<T>): Promise<T> {
	const current = mutationQueues.get(key) ?? Promise.resolve();
	let release!: () => void;
	const next = new Promise<void>((resolve) => {
		release = resolve;
	});
	const chained = current.then(() => next);
	mutationQueues.set(key, chained);

	await current;
	try {
		return await operation();
	} finally {
		release();
		if (mutationQueues.get(key) === chained) mutationQueues.delete(key);
	}
}
