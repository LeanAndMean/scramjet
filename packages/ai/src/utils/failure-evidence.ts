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
export type ResponsesFailureCategory = Exclude<FailureCategory, "missing_body"> | "provider_error";
export type ResponsesRetryDisposition = "transient" | "non_transient" | "unknown";
export interface ResponsesProviderFailureV1 {
	schemaVersion: 1;
	layer: "openai_responses";
	phase: "request" | "stream";
	kind: "http" | "provider_event" | "transport" | "malformed_event" | "stream_termination";
	category: ResponsesFailureCategory;
	retryDisposition: ResponsesRetryDisposition;
	detailSource: "provider_code" | "provider_type" | "http_status" | "message_category" | "none";
	httpStatus?: number;
	providerCode?: (typeof responsesProviderCodes)[number];
}
export type ResponsesProviderFailureValidation =
	| { status: "absent" }
	| { status: "malformed" }
	| { status: "duplicate" }
	| { status: "valid"; category: ResponsesFailureCategory; retryDisposition: ResponsesRetryDisposition };
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
export function httpFailureCategory(status: number): (typeof categories)[number] {
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
		if (d.reason !== "status" && !isFailureCategoryCompatibleWithStatus(d.status as number, d.reason as string))
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
export function responsesHttpFailureCategory(status: number | undefined): ResponsesFailureCategory | undefined {
	return status !== undefined && ([400, 401, 403, 404, 408, 409, 413, 422, 429].includes(status) || status >= 500)
		? httpFailureCategory(status)
		: undefined;
}
export function isFailureCategoryCompatibleWithStatus(status: number | undefined, category: string): boolean {
	if (status === undefined) return true;
	if ([401, 403, 404].includes(status)) return category === httpFailureCategory(status);
	if (category === "context_overflow") return [400, 413, 422].includes(status);
	return !transient.has(category) || status === 408 || status === 429 || status >= 500;
}
export const responsesCategoryDispositions = Object.fromEntries(
	[...categories, "provider_error", "missing_terminal_event", "malformed_event"].map((category) => [
		category,
		transient.has(category)
			? "transient"
			: ["unknown", "provider_error", "missing_terminal_event", "malformed_event"].includes(category)
				? "unknown"
				: "non_transient",
	]),
) as Record<ResponsesFailureCategory, ResponsesRetryDisposition>;
function isResponsesFailureDetails(value: unknown): value is ResponsesProviderFailureV1 {
	const d = object(value);
	if (
		!d ||
		Object.keys(d).some(
			(key) =>
				![
					"schemaVersion",
					"layer",
					"phase",
					"kind",
					"category",
					"retryDisposition",
					"detailSource",
					"httpStatus",
					"providerCode",
				].includes(key),
		)
	)
		return false;
	const category = d.category;
	const kind = d.kind;
	const source = d.detailSource;
	const status = d.httpStatus;
	const code = d.providerCode;
	if (
		d.schemaVersion !== 1 ||
		d.layer !== "openai_responses" ||
		!member(d.phase, ["request", "stream"]) ||
		!member(kind, ["http", "provider_event", "transport", "malformed_event", "stream_termination"]) ||
		typeof category !== "string" ||
		!Object.hasOwn(responsesCategoryDispositions, category) ||
		d.retryDisposition !== responsesCategoryDispositions[category as ResponsesFailureCategory] ||
		!member(source, ["provider_code", "provider_type", "http_status", "message_category", "none"]) ||
		(status !== undefined &&
			(typeof status !== "number" || !Number.isInteger(status) || status < 100 || status > 599)) ||
		(code !== undefined && !member(code, responsesProviderCodes))
	)
		return false;
	if (!isFailureCategoryCompatibleWithStatus(status as number | undefined, category)) return false;
	if (kind === "stream_termination")
		return (
			d.phase === "stream" &&
			category === "missing_terminal_event" &&
			source === "none" &&
			status === undefined &&
			code === undefined
		);
	if (category === "missing_terminal_event") return false;
	if (kind === "malformed_event")
		return category === "malformed_event" && source === "none" && status === undefined && code === undefined;
	if (kind === "transport")
		return (
			status === undefined &&
			code === undefined &&
			(source === "none"
				? category === "transport" || category === "timeout"
				: d.phase === "request" && source === "message_category" && category === "transport")
		);
	if (kind === "http" && status === undefined) return false;
	if (kind === "provider_event" && d.phase === "request" && (status !== undefined || category === "transport"))
		return false;
	if (source === "provider_code" || source === "provider_type")
		return (
			typeof code === "string" &&
			responsesProviderCodeCategories[code as (typeof responsesProviderCodes)[number]] === category
		);
	if (source === "http_status")
		return code === undefined && responsesHttpFailureCategory(status as number | undefined) === category;
	if (source === "message_category") {
		if (["provider_error", "malformed_event", "unknown"].includes(category)) return false;
		if (code !== undefined && (category !== "context_overflow" || code === "context_length_exceeded")) return false;
		return (
			responsesHttpFailureCategory(status as number | undefined) === undefined || category === "context_overflow"
		);
	}
	if (code !== undefined) return false;
	if (kind === "http") {
		const statusCategory = responsesHttpFailureCategory(status as number);
		return (
			(category === "unknown" && statusCategory === undefined) ||
			(category === "provider_error" && (statusCategory === undefined || transient.has(statusCategory)))
		);
	}
	if (status !== undefined)
		return (
			kind === "provider_event" &&
			(category === "provider_error" || category === "unknown") &&
			responsesHttpFailureCategory(status as number) === undefined
		);
	return kind === "provider_event" && ["provider_error", "unknown", "malformed_event"].includes(category);
}
export function validateResponsesProviderFailure(diagnostics: unknown): ResponsesProviderFailureValidation {
	if (diagnostics === undefined) return { status: "absent" };
	if (!Array.isArray(diagnostics)) return { status: "malformed" };
	const matches = diagnostics.filter((entry) => object(entry)?.type === "provider_failure");
	if (matches.length === 0) return { status: "absent" };
	if (matches.length > 1) return { status: "duplicate" };
	const details = object(matches[0])?.details;
	if (!isResponsesFailureDetails(details)) return { status: "malformed" };
	return { status: "valid", category: details.category, retryDisposition: details.retryDisposition };
}
export function blocksFailureRecovery(message: AssistantMessage): boolean {
	if (message.stopReason === "aborted") return true;
	const evidence = inspectFailureEvidence(message.diagnostics);
	if (evidence.status === "valid" && evidence.source === "local" && evidence.category === "context_overflow")
		return false;
	if (message.origin === "harness") return true;
	return (
		evidence.status !== "absent" &&
		(evidence.status !== "valid" ||
			evidence.source !== "provider" ||
			evidence.suppression !== undefined ||
			["unknown", "provider_error", "malformed_event"].includes(evidence.category))
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
	not_found: "not_found",
	conflict: "conflict",
	authentication_error: "authentication",
	permission_denied: "permission",
	invalid_request_error: "invalid_request",
	ValidationException: "invalid_request",
	content_filter: "content_rejection",
	content_policy_violation: "content_rejection",
};
export const responsesProviderCodes = [
	"rate_limit_exceeded",
	"insufficient_quota",
	"billing_hard_limit_reached",
	"overloaded_error",
	"server_error",
	"timeout",
	"context_length_exceeded",
	"authentication_error",
	"permission_denied",
	"invalid_request_error",
	"not_found",
	"conflict",
	"content_filter",
	"content_policy_violation",
] as const;
export const responsesProviderCodeCategories = Object.fromEntries(
	responsesProviderCodes.map((code) => [code, codeCategory[code]]),
) as Record<(typeof responsesProviderCodes)[number], ResponsesFailureCategory>;
export function failureFromProviderError(error: unknown, provider?: string): RequestFailureV1 | undefined {
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
	if (provider === "anthropic" && candidate === "request_too_large" && status === 413) category = "context_overflow";
	if (
		(!category || category === "invalid_request") &&
		(status === 400 || status === 413 || status === 422 || category === "invalid_request") &&
		typeof message === "string" &&
		isProviderOverflowMessage(message, provider)
	)
		category = "context_overflow";
	if (hasStatus && [401, 403, 404].includes(status as number))
		return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (hasStatus && status !== 408 && status !== 429 && (status as number) < 500 && category && transient.has(category))
		return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (category && isFailureCategoryCompatibleWithStatus(hasStatus ? (status as number) : undefined, category))
		return { schemaVersion: 1, kind: "provider", category };
	if (hasStatus) return { schemaVersion: 1, kind: "http", status: status as number, reason: "status" };
	if (typeof code === "string" && code.length > 0) return { schemaVersion: 1, kind: "provider", category: "unknown" };
	return undefined;
}
export function isProviderOverflowMessage(message: string, provider?: string): boolean {
	if (/rate limit|too many requests|throttling|quota|billing/i.test(message)) return false;
	if (
		/prompt is too long|context_length_exceeded|maximum context length|exceeds? (?:the )?context (?:window|length)|input (?:is )?too long|input token count.*exceeds the maximum/i.test(
			message,
		)
	)
		return true;
	if (provider === "anthropic" && /request_too_large/i.test(message)) return true;
	if (provider === "xai" && /maximum prompt length is \d+/i.test(message)) return true;
	if (provider === "groq" && /reduce the length of the messages/i.test(message)) return true;
	if (
		provider === "together" &&
		/input \(\d+ tokens\) is longer than the model'?s context length \(\d+ tokens\)/i.test(message)
	)
		return true;
	return provider === "cerebras" && /^4(?:00|13)\s*(?:status code)?\s*\(no body\)/i.test(message);
}
export function appendBuiltinFailure(message: AssistantMessage, error: unknown, preparationFailed = false): void {
	if (inspectFailureEvidence(message.diagnostics).status !== "absent") return;
	appendObservedFailure(message, error, preparationFailed);
	if (inspectFailureEvidence(message.diagnostics).status === "absent")
		appendRequestFailure(message, { schemaVersion: 1, kind: "provider", category: "unknown" });
}
export function appendObservedFailure(message: AssistantMessage, error: unknown, preparationFailed = false): void {
	const failure: RequestFailureV1 | undefined =
		preparationFailed && !(error instanceof RequestFailureError)
			? { schemaVersion: 1, kind: "local", reason: "request_preparation" }
			: failureFromProviderError(error, message.provider);
	if (failure) appendRequestFailure(message, failure);
	if (error instanceof RequestFailureError && error.suppression)
		message.diagnostics = [
			...(message.diagnostics ?? []),
			{ type: "retry_suppression", timestamp: Date.now(), details: error.suppression },
		];
}
