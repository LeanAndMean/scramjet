import { createHash } from "node:crypto";
import { type ExecOptions, type ExecResult, truncateTail } from "@leanandmean/coding-agent";
import { controlSafeText } from "./text.js";
import type { ForgeRepository } from "./types.js";

export const FORGE_EXEC_TIMEOUT_MS = 3000;
const PROCESS_DIAGNOSTIC_MAX_BYTES = 4096;
const PROCESS_DIAGNOSTIC_MAX_LINES = 40;

export const FORGE_AUTH_FAILURE_PATTERNS = [
	/\bnot logged (?:in|into)\b/i,
	/\bno hosts? (?:are )?configured\b/i,
	/\bno (?:authentication |auth )?token (?:is )?found\b/i,
	/\btoken\b[^\n]*(?:invalid|expired|revoked)\b/i,
	/(?:invalid|expired|revoked)[^\n]*\btoken\b/i,
	/\bauthentication (?:failed|required)\b/i,
	/\bunauthorized\b/i,
	/\bHTTP(?:\/\S+)?\s+401\b/i,
];

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

export interface ForgeProcessDiagnostic {
	exitCode: number | null;
	stdout: string;
	stderr: string;
	spawnError?: { message: string; code?: string };
	stdinError?: { message: string; code?: string };
	authenticationFailure?: boolean;
}

export interface ForgeInvocationDiagnostic {
	command: string;
	args: string[];
	cwd: string;
	stdin?: { bytes: number; sha256: string };
	process?: ForgeProcessDiagnostic;
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

export class UnsupportedForgeOriginError extends Error {
	constructor() {
		super("Origin is not a supported GitHub or GitLab origin");
		this.name = "UnsupportedForgeOriginError";
	}
}

function quotedDiagnostic(value: string): string {
	return JSON.stringify(value).replace(/\\\\u([0-9A-F]{4})/g, (_match, hex: string) => {
		const rendered = Number.parseInt(hex, 16) < 0x20 ? hex.toLowerCase() : hex;
		return `\\u${rendered}`;
	});
}

function summarizedArgument(value: string): string {
	if (!value.startsWith("query=")) return value;
	return `query=<sha256:${createHash("sha256").update(value, "utf8").digest("hex").slice(0, 12)}>`;
}

function messageForFailure(kind: ForgeCommandErrorKind, invocation: ForgeInvocationDiagnostic): string {
	const command = [invocation.command, ...invocation.args.map(summarizedArgument)].map(quotedDiagnostic).join(" ");
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
	const process = invocation.process;
	const processText = process
		? [
				process.exitCode === null ? null : `exit code ${process.exitCode}`,
				process.spawnError === undefined
					? null
					: `spawn error${process.spawnError.code ? ` ${process.spawnError.code}` : ""} ${quotedDiagnostic(process.spawnError.message)}`,
				process.stdinError === undefined
					? null
					: `stdin error${process.stdinError.code ? ` ${process.stdinError.code}` : ""} ${quotedDiagnostic(process.stdinError.message)}`,
				process.stdout === "" ? null : `stdout ${quotedDiagnostic(process.stdout)}`,
				process.stderr === "" ? null : `stderr ${quotedDiagnostic(process.stderr)}`,
			].filter((part): part is string => part !== null)
		: [];
	const diagnostic = processText.length === 0 ? "" : `; ${processText.join("; ")}`;
	const login =
		invocation.command === "gh"
			? "Run `gh auth login --hostname github.com`"
			: invocation.command === "glab"
				? "Run `glab auth login --hostname gitlab.com`"
				: "";
	const guidance =
		processAuthenticationFailure(process) && login !== ""
			? invocation.stdin === undefined
				? ` ${login}, then retry the read after confirming authentication.`
				: ` ${login}. Authentication recovery does not establish whether the mutation occurred.`
			: "";
	return `${reason}: ${command}${stdin}${diagnostic}.${guidance}`;
}

function processAuthenticationFailure(process: ForgeProcessDiagnostic | undefined): boolean {
	if (process === undefined) return false;
	if (process.authenticationFailure !== undefined) return process.authenticationFailure;
	const diagnostic = `${process.stdout}\n${process.stderr}`;
	return FORGE_AUTH_FAILURE_PATTERNS.some((pattern) => pattern.test(diagnostic));
}

function redactionValues(stdin: string | undefined): string[] {
	if (stdin === undefined) return [];
	const values = new Set<string>([stdin]);
	try {
		const visit = (value: unknown) => {
			if (typeof value === "string") {
				if (value !== "") {
					values.add(value);
					values.add(JSON.stringify(value).slice(1, -1));
				}
				return;
			}
			if (Array.isArray(value)) {
				for (const item of value) visit(item);
				return;
			}
			if (typeof value === "object" && value !== null) {
				for (const item of Object.values(value)) visit(item);
			}
		};
		visit(JSON.parse(stdin));
	} catch {}
	return [...values].sort((left, right) => right.length - left.length);
}

function redactProcessOutput(value: string, secrets: readonly string[]): string {
	let redacted = value;
	for (const secret of secrets) redacted = redacted.replaceAll(secret, "[redacted]");
	return redacted.replace(/"(?:\\.|[^"\\])*"/g, (token) => {
		let decoded: unknown;
		try {
			decoded = JSON.parse(token);
		} catch {
			return token;
		}
		return typeof decoded === "string" && secrets.some((secret) => decoded.includes(secret)) ? '"[redacted]"' : token;
	});
}

function boundedProcessOutput(value: string): string {
	const options = {
		maxBytes: PROCESS_DIAGNOSTIC_MAX_BYTES,
		maxLines: PROCESS_DIAGNOSTIC_MAX_LINES,
	};
	const truncated = truncateTail(value, options);
	if (!truncated.truncated) return truncated.content;
	const prefix = "[truncated] ";
	return `${prefix}${
		truncateTail(value, {
			...options,
			maxBytes: options.maxBytes - Buffer.byteLength(prefix, "utf8"),
		}).content
	}`;
}

function suppressedProcessOutput(value: string): string {
	if (value === "") return "";
	return `[suppressed ${Buffer.byteLength(value, "utf8")} bytes; sha256 ${createHash("sha256").update(value, "utf8").digest("hex")}]`;
}

function authenticationFailure(output: string, stdin: string | undefined): boolean {
	return FORGE_AUTH_FAILURE_PATTERNS.some((pattern) => {
		const match = output.match(pattern)?.[0];
		return match !== undefined && !stdin?.toLowerCase().includes(match.toLowerCase());
	});
}

function diagnosticFor(invocation: ForgeInvocation, result?: ExecResult): ForgeInvocationDiagnostic {
	const secrets = redactionValues(invocation.stdin);
	const rawStdout = result === undefined ? "" : redactProcessOutput(result.stdout, secrets);
	const rawStderr = result === undefined ? "" : redactProcessOutput(result.stderr, secrets);
	const stdout =
		invocation.stdin === undefined
			? boundedProcessOutput(controlSafeText(rawStdout))
			: suppressedProcessOutput(rawStdout);
	const stderr =
		invocation.stdin === undefined
			? boundedProcessOutput(controlSafeText(rawStderr))
			: suppressedProcessOutput(rawStderr);
	return {
		command: controlSafeText(invocation.command),
		args: invocation.args.map(controlSafeText),
		cwd: controlSafeText(invocation.cwd),
		...(invocation.stdin === undefined
			? {}
			: {
					stdin: {
						bytes: Buffer.byteLength(invocation.stdin, "utf8"),
						sha256: createHash("sha256").update(invocation.stdin, "utf8").digest("hex"),
					},
				}),
		...(result === undefined
			? {}
			: {
					process: {
						exitCode: result.code,
						stdout,
						stderr,
						...(result.spawnError === undefined
							? {}
							: {
									spawnError: {
										message: boundedProcessOutput(controlSafeText(result.spawnError.message)),
										...(result.spawnError.code === undefined
											? {}
											: { code: controlSafeText(result.spawnError.code) }),
									},
								}),
						...(result.stdinError === undefined
							? {}
							: {
									stdinError: {
										message: boundedProcessOutput(controlSafeText(result.stdinError.message)),
										...(result.stdinError.code === undefined
											? {}
											: { code: controlSafeText(result.stdinError.code) }),
									},
								}),
						authenticationFailure: authenticationFailure(`${rawStdout}\n${rawStderr}`, invocation.stdin),
					},
				}),
	};
}

export async function runForgeCommandResult(exec: ForgeExec, invocation: ForgeInvocation): Promise<ExecResult> {
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

	const processDiagnostic = diagnosticFor(invocation, result);
	if (result.spawnError?.code === "ENOENT") throw new ForgeCommandError("missing-executable", processDiagnostic);
	if (result.killed && invocation.signal?.aborted) throw new ForgeCommandError("cancelled", processDiagnostic);
	if (result.killed) throw new ForgeCommandError("timeout", processDiagnostic);
	if (result.stdinError) throw new ForgeCommandError("stdin", processDiagnostic);
	if (result.spawnError) throw new ForgeCommandError("failed", processDiagnostic);
	return result;
}

export async function runForgeCommand(exec: ForgeExec, invocation: ForgeInvocation): Promise<ExecResult> {
	const result = await runForgeCommandResult(exec, invocation);
	if (result.code !== 0) throw new ForgeCommandError("failed", diagnosticFor(invocation, result));
	return result;
}

function invalidRemote(): never {
	throw new UnsupportedForgeOriginError();
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
