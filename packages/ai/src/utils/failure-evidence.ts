import { validateResponsesProviderFailure } from "../providers/openai-responses-shared.js";
import type { AssistantMessage } from "../types.js";

// SCRAMJET-DIVERGENCE: Failure facts survive persistence independently of recovery policy.
const categories = [
	"rate_limit",
	"quota_exhausted",
	"overloaded",
	"server",
	"timeout",
	"transport",
	"context_overflow",
	"authentication",
	"permission",
	"invalid_request",
	"not_found",
	"conflict",
	"content_rejection",
	"unknown",
] as const;
export type FailureCategory =
	| (typeof categories)[number]
	| "missing_body"
	| "missing_terminal_event"
	| "malformed_event";
export type RequestFailureV1 =
	| {
			schemaVersion: 1;
			kind: "http";
			status: number;
			reason:
				| "status"
				| "quota_exhausted"
				| "context_overflow"
				| "authentication"
				| "permission"
				| "invalid_request"
				| "content_rejection"
				| "unknown";
	  }
	| { schemaVersion: 1; kind: "provider"; category: (typeof categories)[number] }
	| {
			schemaVersion: 1;
			kind: "stream";
			reason: "missing_body" | "missing_terminal_event" | "transport" | "timeout" | "malformed_event";
	  }
	| { schemaVersion: 1; kind: "callback"; callback: "onPayload" | "onResponse" }
	| {
			schemaVersion: 1;
			kind: "local";
			reason: "request_preparation" | "request_validation" | "configuration" | "context_overflow";
	  };
export type RetrySuppressionV1 =
	| { schemaVersion: 1; reason: "server_delay_exceeds_limit"; requestedDelayMs: number; maxDelayMs: number }
	| { schemaVersion: 1; reason: "invalid_server_delay" };
export type FailureEvidence =
	| { status: "absent" }
	| { status: "malformed" | "duplicate" | "conflicting" }
	| {
			status: "valid";
			family: "provider_failure" | "request_failure";
			category: string;
			source: "provider" | "callback" | "local";
			transient: boolean;
			suppression?: RetrySuppressionV1["reason"];
	  };
const transient = new Set<string>(["rate_limit", "overloaded", "server", "timeout", "transport"]);
const object = (value: unknown): Record<string, unknown> | undefined =>
	value !== null && typeof value === "object" && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: undefined;
const keys = (value: Record<string, unknown>, names: string[]) =>
	Object.keys(value).length === names.length && names.every((key) => Object.hasOwn(value, key));
const member = (value: unknown, choices: readonly string[]) => typeof value === "string" && choices.includes(value);
export function httpFailureCategory(status: number): FailureCategory {
	if (status === 408 || status === 504) return "timeout";
	if (status === 429) return "rate_limit";
	if (status >= 500) return "server";
	if (status === 401) return "authentication";
	if (status === 403) return "permission";
	if (status === 404) return "not_found";
	if (status === 409) return "conflict";
	return "invalid_request";
}
export function validateRequestFailure(diagnostics: unknown): FailureEvidence {
	if (diagnostics === undefined) return { status: "absent" };
	if (!Array.isArray(diagnostics)) return { status: "malformed" };
	const matches = diagnostics.filter((entry) => object(entry)?.type === "request_failure");
	if (matches.length === 0) return { status: "absent" };
	if (matches.length > 1) return { status: "duplicate" };
	const d = object(matches[0].details);
	if (!d || d.schemaVersion !== 1) return { status: "malformed" };
	let category: string;
	let source: "provider" | "callback" | "local" = "provider";
	if (
		d.kind === "http" &&
		keys(d, ["schemaVersion", "kind", "status", "reason"]) &&
		Number.isInteger(d.status) &&
		(d.status as number) >= 400 &&
		(d.status as number) <= 599 &&
		member(d.reason, [
			"status",
			"quota_exhausted",
			"context_overflow",
			"authentication",
			"permission",
			"invalid_request",
			"content_rejection",
			"unknown",
		])
	) {
		if (
			[401, 403, 404].includes(d.status as number) &&
			d.reason !== "status" &&
			d.reason !== httpFailureCategory(d.status as number)
		)
			return { status: "malformed" };
		category = d.reason === "status" ? httpFailureCategory(d.status as number) : (d.reason as string);
	} else if (
		d.kind === "provider" &&
		keys(d, ["schemaVersion", "kind", "category"]) &&
		member(d.category, categories)
	) {
		category = d.category as string;
	} else if (
		d.kind === "stream" &&
		keys(d, ["schemaVersion", "kind", "reason"]) &&
		member(d.reason, ["missing_body", "missing_terminal_event", "transport", "timeout", "malformed_event"])
	) {
		category = d.reason as string;
	} else if (
		d.kind === "callback" &&
		keys(d, ["schemaVersion", "kind", "callback"]) &&
		member(d.callback, ["onPayload", "onResponse"])
	) {
		source = "callback";
		category = "callback";
	} else if (
		d.kind === "local" &&
		keys(d, ["schemaVersion", "kind", "reason"]) &&
		member(d.reason, ["request_preparation", "request_validation", "configuration", "context_overflow"])
	) {
		source = "local";
		category = d.reason === "context_overflow" ? "context_overflow" : "local";
	} else return { status: "malformed" };
	return { status: "valid", family: "request_failure", category, source, transient: transient.has(category) };
}
export function inspectFailureEvidence(diagnostics: unknown): FailureEvidence {
	const request = validateRequestFailure(diagnostics);
	const responses = validateResponsesProviderFailure(diagnostics);
	if (request.status !== "absent" && responses.status !== "absent") return { status: "conflicting" };
	let result: FailureEvidence = request;
	if (responses.status === "valid")
		result = {
			status: "valid",
			family: "provider_failure",
			category: responses.category,
			source: "provider",
			transient: responses.retryDisposition === "transient",
		};
	else if (responses.status !== "absent") result = { status: responses.status };
	const suppressions = Array.isArray(diagnostics)
		? diagnostics.filter((d) => object(d)?.type === "retry_suppression")
		: [];
	if (!suppressions.length) return result;
	if (suppressions.length > 1) return { status: "duplicate" };
	if (result.status !== "valid" || !result.transient || result.source !== "provider") return { status: "malformed" };
	const d = object(suppressions[0].details);
	if (!d || d.schemaVersion !== 1) return { status: "malformed" };
	if (d.reason === "invalid_server_delay" && keys(d, ["schemaVersion", "reason"]))
		return { ...result, suppression: d.reason };
	if (
		d.reason === "server_delay_exceeds_limit" &&
		keys(d, ["schemaVersion", "reason", "requestedDelayMs", "maxDelayMs"]) &&
		typeof d.requestedDelayMs === "number" &&
		Number.isFinite(d.requestedDelayMs) &&
		d.requestedDelayMs <= Number.MAX_SAFE_INTEGER &&
		typeof d.maxDelayMs === "number" &&
		Number.isFinite(d.maxDelayMs) &&
		d.maxDelayMs > 0 &&
		d.requestedDelayMs > d.maxDelayMs
	)
		return { ...result, suppression: d.reason };
	return { status: "malformed" };
}
export function blocksFailureRecovery(message: AssistantMessage): boolean {
	if (message.stopReason === "aborted") return true;
	const evidence = inspectFailureEvidence(message.diagnostics);
	if (evidence.status === "valid" && evidence.source === "local" && evidence.category === "context_overflow")
		return false;
	if (message.origin === "harness") return true;
	return (
		evidence.status !== "absent" &&
		(evidence.status !== "valid" || evidence.source !== "provider" || evidence.suppression !== undefined)
	);
}
export function appendRequestFailure(message: AssistantMessage, details: RequestFailureV1): void {
	message.diagnostics = [...(message.diagnostics ?? []), { type: "request_failure", timestamp: Date.now(), details }];
}
export class RequestFailureError extends Error {
	constructor(
		message: string,
		readonly failure: RequestFailureV1,
		readonly suppression?: RetrySuppressionV1,
	) {
		super(message);
	}
}
export async function invokeProviderCallback<T>(
	name: "onPayload" | "onResponse",
	callback: () => T | Promise<T>,
): Promise<T> {
	try {
		return await callback();
	} catch {
		throw new RequestFailureError(`Provider ${name} callback failed.`, {
			schemaVersion: 1,
			kind: "callback",
			callback: name,
		});
	}
}
const codeCategory: Record<string, (typeof categories)[number]> = {
	usage_limit_reached: "quota_exhausted",
	usage_not_included: "quota_exhausted",
	insufficient_quota: "quota_exhausted",
	billing_hard_limit_reached: "quota_exhausted",
	rate_limit_exceeded: "rate_limit",
	ThrottlingException: "rate_limit",
	overloaded_error: "overloaded",
	server_error: "server",
	InternalServerException: "server",
	ServiceUnavailableException: "server",
	ECONNRESET: "transport",
	ECONNREFUSED: "transport",
	EPIPE: "transport",
	EAI_AGAIN: "transport",
	UND_ERR_SOCKET: "transport",
	ETIMEDOUT: "timeout",
	UND_ERR_CONNECT_TIMEOUT: "timeout",
	UND_ERR_HEADERS_TIMEOUT: "timeout",
	UND_ERR_BODY_TIMEOUT: "timeout",
	TimeoutError: "timeout",
	APIConnectionError: "transport",
	APIConnectionTimeoutError: "timeout",
	timeout: "timeout",
	ModelTimeoutException: "timeout",
	context_length_exceeded: "context_overflow",
	authentication_error: "authentication",
	permission_denied: "permission",
	invalid_request_error: "invalid_request",
	ValidationException: "invalid_request",
	content_filter: "content_rejection",
	content_policy_violation: "content_rejection",
};
export function failureFromProviderError(error: unknown): RequestFailureV1 | undefined {
	if (error instanceof RequestFailureError) return error.failure;
	const e = object(error);
	if (!e) return undefined;
	let nested = object(e.error);
	const metadata = object(e.$metadata);
	const status = e.status ?? e.statusCode ?? metadata?.httpStatusCode;
	const hasStatus = typeof status === "number" && Number.isInteger(status) && status >= 400 && status <= 599;
	if (!nested && hasStatus) {
		for (const value of [e.body, e.message]) {
			if (typeof value !== "string") continue;
			try {
				nested = object(object(JSON.parse(value))?.error);
			} catch {
				/* Not a JSON error body. */
			}
			if (nested) break;
		}
	}
	const code = nested?.code ?? nested?.type ?? e.code ?? object(e.cause)?.code;
	const candidate = typeof code === "string" ? code : e.name;
	let category =
		typeof candidate === "string" && Object.hasOwn(codeCategory, candidate) ? codeCategory[candidate] : undefined;
	const message = nested?.message ?? e.message;
	if (
		(status === 400 || status === 413 || category === "invalid_request") &&
		typeof message === "string" &&
		/prompt is too long|context_length_exceeded|maximum context length|exceeds? (?:the )?context (?:window|length)|input (?:is )?too long|input token count.*exceeds the maximum/i.test(
			message,
		)
	)
		category = "context_overflow";
	if (hasStatus && [401, 403, 404].includes(status as number))
		return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (hasStatus && status !== 408 && status !== 429 && (status as number) < 500 && category && transient.has(category))
		return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (category) return { schemaVersion: 1, kind: "provider", category };
	if (hasStatus) return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (typeof code === "string" && code.length > 0) return { schemaVersion: 1, kind: "provider", category: "unknown" };
	return undefined;
}
export function appendObservedFailure(message: AssistantMessage, error: unknown, preparationFailed = false): void {
	const failure: RequestFailureV1 | undefined =
		preparationFailed && !(error instanceof RequestFailureError)
			? { schemaVersion: 1, kind: "local", reason: "request_preparation" }
			: failureFromProviderError(error);
	if (failure) appendRequestFailure(message, failure);
	if (error instanceof RequestFailureError && error.suppression)
		message.diagnostics = [
			...(message.diagnostics ?? []),
			{ type: "retry_suppression", timestamp: Date.now(), details: error.suppression },
		];
}
