import type OpenAI from "openai";
import { APIConnectionError, APIConnectionTimeoutError, APIError, APIUserAbortError } from "openai";
import type {
	Tool as OpenAITool,
	ResponseCreateParamsStreaming,
	ResponseFunctionCallOutputItemList,
	ResponseFunctionToolCall,
	ResponseInput,
	ResponseInputContent,
	ResponseInputImage,
	ResponseInputText,
	ResponseOutputMessage,
	ResponseReasoningItem,
	ResponseStreamEvent,
} from "openai/resources/responses/responses.js";
import { calculateCost } from "../models.js";
import type {
	Api,
	AssistantMessage,
	Context,
	ImageContent,
	Model,
	StopReason,
	TextContent,
	TextSignatureV1,
	ThinkingContent,
	Tool,
	ToolCall,
	Usage,
} from "../types.js";
import type { AssistantMessageEventStream } from "../utils/event-stream.js";
import { shortHash } from "../utils/hash.js";
import { parseStreamingJson } from "../utils/json-parse.js";
import { sanitizeSurrogates } from "../utils/sanitize-unicode.js";
import { flattenSystemPrompt } from "../utils/system-prompt.js";
import { transformMessages } from "./transform-messages.js";

// =============================================================================
// Utilities
// =============================================================================

function encodeTextSignatureV1(id: string, phase?: TextSignatureV1["phase"]): string {
	const payload: TextSignatureV1 = { v: 1, id };
	if (phase) payload.phase = phase;
	return JSON.stringify(payload);
}

function parseTextSignature(
	signature: string | undefined,
): { id: string; phase?: TextSignatureV1["phase"] } | undefined {
	if (!signature) return undefined;
	if (signature.startsWith("{")) {
		try {
			const parsed = JSON.parse(signature) as Partial<TextSignatureV1>;
			if (parsed.v === 1 && typeof parsed.id === "string") {
				if (parsed.phase === "commentary" || parsed.phase === "final_answer") {
					return { id: parsed.id, phase: parsed.phase };
				}
				return { id: parsed.id };
			}
		} catch {
			// Fall through to legacy plain-string handling.
		}
	}
	return { id: signature };
}

export interface OpenAIResponsesStreamOptions {
	serviceTier?: ResponseCreateParamsStreaming["service_tier"];
	resolveServiceTier?: (
		responseServiceTier: ResponseCreateParamsStreaming["service_tier"] | undefined,
		requestServiceTier: ResponseCreateParamsStreaming["service_tier"] | undefined,
	) => ResponseCreateParamsStreaming["service_tier"] | undefined;
	applyServiceTierPricing?: (
		usage: Usage,
		serviceTier: ResponseCreateParamsStreaming["service_tier"] | undefined,
	) => void;
}

export interface ConvertResponsesMessagesOptions {
	includeSystemPrompt?: boolean;
}

export interface ConvertResponsesToolsOptions {
	strict?: boolean | null;
}

// SCRAMJET-DIVERGENCE: Shared Responses failures use closed retry diagnostics and bounded local error text (#553, #575).
export type ResponsesFailureCategory =
	| "rate_limit"
	| "quota_exhausted"
	| "overloaded"
	| "server"
	| "timeout"
	| "transport"
	| "context_overflow"
	| "authentication"
	| "permission"
	| "invalid_request"
	| "not_found"
	| "conflict"
	| "content_rejection"
	| "provider_error"
	| "malformed_event"
	| "unknown";

export type ResponsesRetryDisposition = "transient" | "non_transient" | "unknown";

type ResponsesFailureKind = "http" | "provider_event" | "transport" | "malformed_event";
type ResponsesFailureDetailSource = "provider_code" | "provider_type" | "http_status" | "message_category" | "none";

type ResponsesProviderCode =
	| "rate_limit_exceeded"
	| "insufficient_quota"
	| "billing_hard_limit_reached"
	| "overloaded_error"
	| "server_error"
	| "timeout"
	| "context_length_exceeded"
	| "authentication_error"
	| "permission_denied"
	| "invalid_request_error"
	| "not_found"
	| "conflict"
	| "content_filter"
	| "content_policy_violation";

export interface ResponsesProviderFailureV1 {
	schemaVersion: 1;
	layer: "openai_responses";
	phase: "request" | "stream";
	kind: ResponsesFailureKind;
	category: ResponsesFailureCategory;
	retryDisposition: ResponsesRetryDisposition;
	detailSource: ResponsesFailureDetailSource;
	httpStatus?: number;
	providerCode?: ResponsesProviderCode;
}

export type ResponsesProviderFailureValidation =
	| { status: "absent" }
	| {
			status: "valid";
			category: ResponsesFailureCategory;
			retryDisposition: ResponsesRetryDisposition;
	  }
	| { status: "malformed" }
	| { status: "duplicate" };

const PROVIDER_CODE_CATEGORIES = {
	rate_limit_exceeded: "rate_limit",
	insufficient_quota: "quota_exhausted",
	billing_hard_limit_reached: "quota_exhausted",
	overloaded_error: "overloaded",
	server_error: "server",
	timeout: "timeout",
	context_length_exceeded: "context_overflow",
	authentication_error: "authentication",
	permission_denied: "permission",
	invalid_request_error: "invalid_request",
	not_found: "not_found",
	conflict: "conflict",
	content_filter: "content_rejection",
	content_policy_violation: "content_rejection",
} as const satisfies Record<ResponsesProviderCode, ResponsesFailureCategory>;

const CATEGORY_DISPOSITIONS: Record<ResponsesFailureCategory, ResponsesRetryDisposition> = {
	rate_limit: "transient",
	quota_exhausted: "non_transient",
	overloaded: "transient",
	server: "transient",
	timeout: "transient",
	transport: "transient",
	context_overflow: "non_transient",
	authentication: "non_transient",
	permission: "non_transient",
	invalid_request: "non_transient",
	not_found: "non_transient",
	conflict: "non_transient",
	content_rejection: "non_transient",
	provider_error: "unknown",
	malformed_event: "unknown",
	unknown: "unknown",
};

const CATEGORY_MESSAGES: Record<ResponsesFailureCategory, string> = {
	rate_limit: "OpenAI Responses request was rate limited.",
	quota_exhausted: "OpenAI Responses quota was exhausted.",
	overloaded: "OpenAI Responses service was overloaded.",
	server: "OpenAI Responses service returned a server error.",
	timeout: "OpenAI Responses request timed out.",
	transport: "OpenAI Responses request failed during transport.",
	context_overflow: "OpenAI Responses input exceeds the context window.",
	authentication: "OpenAI Responses authentication failed.",
	permission: "OpenAI Responses request was not permitted.",
	invalid_request: "OpenAI Responses request was invalid.",
	not_found: "OpenAI Responses resource was not found.",
	conflict: "OpenAI Responses request conflicted with the current resource state.",
	content_rejection: "OpenAI Responses rejected the content.",
	provider_error: "OpenAI Responses returned a provider error.",
	malformed_event: "OpenAI Responses returned a malformed error event.",
	unknown: "OpenAI Responses request failed without recognized details.",
};

const PROVIDER_MESSAGE_MAX_LENGTH = 4096;

const GATEWAY_OBSERVABILITY: GatewayObservabilityV1 = {
	schemaVersion: 1,
	layer: "gateway_service_internal",
	outcome: "unobservable",
	reason: "no_structured_evidence",
};

const PROVIDER_CODES = new Set<string>(Object.keys(PROVIDER_CODE_CATEGORIES));
const FAILURE_DETAIL_KEYS = new Set([
	"schemaVersion",
	"layer",
	"phase",
	"kind",
	"category",
	"retryDisposition",
	"detailSource",
	"httpStatus",
	"providerCode",
]);

interface FailureScalars {
	code?: string;
	type?: string;
	message?: string;
	status?: number;
}

export interface SafeResponsesFailure {
	message: string;
	diagnostic: ResponsesProviderFailureV1;
}

type ResponsesSdkRetryAttempt =
	| { ordinal: number; result: "response"; status?: number }
	| { ordinal: number; result: "transport"; category: "timeout" | "connection" | "other" };

type ResponsesSdkRetryReason =
	| "accepted_after_retry"
	| "configured_limit_reached"
	| "terminal_after_retry"
	| "configured_zero"
	| "non_retryable_request"
	| "stream_already_accepted"
	| "insufficient_evidence";

export interface GatewayObservabilityV1 {
	schemaVersion: 1;
	layer: "gateway_service_internal";
	outcome: "unobservable";
	reason: "no_structured_evidence";
}

export interface ResponsesSdkRetryV1 {
	schemaVersion: 1;
	layer: "openai_sdk_request";
	outcome: "recovered" | "exhausted" | "not_attempted";
	reason: ResponsesSdkRetryReason;
	observedAttemptCount: number;
	attempts: ResponsesSdkRetryAttempt[];
	truncated: boolean;
}

export interface ResponsesSdkRequestObserver {
	fetch: typeof fetch;
	markAccepted(): void;
	diagnosticForSuccess(): ResponsesSdkRetryV1 | undefined;
	diagnosticForFailure(maxRetries: number | undefined): ResponsesSdkRetryV1;
}

class SafeResponsesFailureError extends Error {
	constructor(readonly failure: SafeResponsesFailure) {
		super(failure.message);
	}
}

function recordOf(value: unknown): Record<string, unknown> | undefined {
	return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : undefined;
}

function finiteString(value: unknown): string | undefined {
	return typeof value === "string" && value.length > 0 && value.length <= 256 ? value : undefined;
}

function boundedMessage(value: unknown): string | undefined {
	return typeof value === "string" && value.length > 0 ? value.slice(0, PROVIDER_MESSAGE_MAX_LENGTH + 1) : undefined;
}

function finiteStatus(value: unknown): number | undefined {
	return typeof value === "number" && Number.isInteger(value) && value >= 100 && value <= 599 ? value : undefined;
}

function readFailureScalars(value: unknown): { top: FailureScalars; nested: FailureScalars } {
	const topRecord = recordOf(value);
	const nestedRecord = recordOf(topRecord?.error);
	const read = (record: Record<string, unknown> | undefined): FailureScalars => ({
		code: finiteString(record?.code),
		type: finiteString(record?.type),
		message:
			record === topRecord &&
			value instanceof APIError &&
			value.error &&
			!(typeof recordOf(value.error)?.message === "string" && recordOf(value.error)?.message)
				? undefined
				: boundedMessage(record?.message),
		status: finiteStatus(record?.status),
	});
	return { top: read(topRecord), nested: read(nestedRecord) };
}

function readableFailureDetail(message: string | undefined): string | undefined {
	const detail = message
		?.replace(/[\u0000-\u001f\u007f-\u009f]/g, " ")
		.replace(/\s+/g, " ")
		.trim();
	if (!detail || /^(?:undefined|null)$/i.test(detail)) return undefined;
	return detail.length > PROVIDER_MESSAGE_MAX_LENGTH ? `${detail.slice(0, PROVIDER_MESSAGE_MAX_LENGTH - 1)}…` : detail;
}

function unfamiliarFieldNames(record: Record<string, unknown> | undefined, known: readonly string[]): string[] {
	return Object.keys(record ?? {})
		.filter((key) => !known.includes(key) && /^[a-z_][a-z0-9_.-]{0,63}$/i.test(key))
		.slice(0, 4);
}

function composeFailureMessage(category: ResponsesFailureCategory, detail: string | undefined): string {
	const base = CATEGORY_MESSAGES[category];
	if (!detail) return base;
	return `${base.slice(0, -1)}: ${detail}${/[.!?…]$/.test(detail) ? "" : "."}`;
}

// SCRAMJET-DIVERGENCE: preserve a readable local abort reason without terminal controls (#553, #575).
export function abortedResponsesFailureMessage(error: unknown): string {
	return (
		readableFailureDetail(error instanceof Error ? error.message : undefined) ??
		"OpenAI Responses request was aborted."
	);
}

function categoryFromMessage(message: string | undefined): ResponsesFailureCategory | undefined {
	if (!message) return undefined;
	const normalized = message.toLowerCase();
	if (/context (length|window)|maximum context|too many tokens/.test(normalized)) return "context_overflow";
	if (/insufficient.quota|quota.*(exhaust|exceed)|billing.*limit/.test(normalized)) return "quota_exhausted";
	if (/rate.?limit|too many requests/.test(normalized)) return "rate_limit";
	if (/overload|capacity/.test(normalized)) return "overloaded";
	if (/timed? ?out|timeout/.test(normalized)) return "timeout";
	if (/connection error|network error/.test(normalized)) return "transport";
	if (/authenticat|invalid api key/.test(normalized)) return "authentication";
	if (/permission|forbidden|not authorized/.test(normalized)) return "permission";
	if (/not found/.test(normalized)) return "not_found";
	if (/conflict/.test(normalized)) return "conflict";
	if (/content.filter|content.policy|safety policy/.test(normalized)) return "content_rejection";
	if (/invalid request|bad request/.test(normalized)) return "invalid_request";
	if (/server error|internal error|internal server/.test(normalized)) return "server";
	if (
		/\bupstream (?:error|unavailable|connect(?:ion)?|request failed)\b|(?:status|http|error|code|returned|received|upstream)\s*:?\s*50[0234]\b|service unavailable|bad gateway|gateway time-?out/.test(
			normalized,
		)
	) {
		return "server";
	}
	if (/fetch failed|socket hang up|econnreset|stream ended/.test(normalized)) return "transport";
	return undefined;
}

function categoryFromStatus(status: number | undefined): ResponsesFailureCategory | undefined {
	if (status === 400 || status === 422) return "invalid_request";
	if (status === 401) return "authentication";
	if (status === 403) return "permission";
	if (status === 404) return "not_found";
	if (status === 409) return "conflict";
	if (status === 408 || status === 504) return "timeout";
	if (status === 429) return "rate_limit";
	if (status !== undefined && status >= 500) return "server";
	return undefined;
}

function allowlistedProviderCode(value: string | undefined): ResponsesProviderCode | undefined {
	return value && PROVIDER_CODES.has(value) ? (value as ResponsesProviderCode) : undefined;
}

function makeFailure(
	value: unknown,
	phase: "request" | "stream",
	kindHint?: ResponsesFailureKind,
): SafeResponsesFailure {
	if (value instanceof SafeResponsesFailureError) return value.failure;
	const { top, nested } = readFailureScalars(value);
	const codeCandidates = [
		[top.code, "provider_code"],
		[top.type, "provider_type"],
		[nested.code, "provider_code"],
		[nested.type, "provider_type"],
	] as const;
	const matchedCode = codeCandidates.find(([value]) => allowlistedProviderCode(value) !== undefined);
	const providerCode = allowlistedProviderCode(matchedCode?.[0]);
	const conflictingCode = codeCandidates.some(
		([code]) => code && code !== providerCode && code !== "error" && code !== "response.failed",
	);
	const contextOverflow = [top.code, top.type, nested.code, nested.type, top.message, nested.message].some(
		(field) => field === "context_length_exceeded" || categoryFromMessage(field) === "context_overflow",
	);
	const status = top.status ?? nested.status;
	const statusCategory = categoryFromStatus(status);
	const messageCategory = categoryFromMessage(top.message) ?? categoryFromMessage(nested.message);
	const sdkConnection = value instanceof APIConnectionError;
	const sdkAbort = value instanceof APIUserAbortError;
	const cause = recordOf(recordOf(value)?.cause);
	const transportCause = ["ECONNRESET", "EPIPE", "ETIMEDOUT", "UND_ERR_SOCKET"].includes(
		finiteString(cause?.code) ?? "",
	);
	const unsupportedEvidence = Boolean(
		Object.keys(recordOf(value) ?? {}).some(
			(key) =>
				![
					"code",
					"type",
					"message",
					"status",
					"param",
					"error",
					"headers",
					"requestID",
					"cause",
					"stack",
					"name",
					"id",
					"sequence_number",
				].includes(key),
		) ||
			Object.keys(recordOf(recordOf(value)?.error) ?? {}).some(
				(key) => !["code", "type", "message", "status", "param"].includes(key),
			) ||
			(top.code && !providerCode) ||
			(top.type && top.type !== "error" && top.type !== "response.failed" && !providerCode) ||
			(nested.code && !providerCode) ||
			(nested.type && !providerCode) ||
			["param", "reason", "incomplete_details"].some((key) => recordOf(value)?.[key] !== undefined),
	);
	let category: ResponsesFailureCategory;
	let detailSource: ResponsesFailureDetailSource;
	if (contextOverflow) {
		category = "context_overflow";
		detailSource = providerCode === "context_length_exceeded" ? "provider_code" : "message_category";
	} else if (
		statusCategory &&
		CATEGORY_DISPOSITIONS[statusCategory] === "non_transient" &&
		providerCode &&
		CATEGORY_DISPOSITIONS[PROVIDER_CODE_CATEGORIES[providerCode]] === "transient"
	) {
		category = statusCategory;
		detailSource = "http_status";
	} else if (
		conflictingCode &&
		((providerCode && CATEGORY_DISPOSITIONS[PROVIDER_CODE_CATEGORIES[providerCode]] === "transient") ||
			(statusCategory && CATEGORY_DISPOSITIONS[statusCategory] === "transient"))
	) {
		category = "provider_error";
		detailSource = "none";
	} else if (providerCode) {
		category = PROVIDER_CODE_CATEGORIES[providerCode];
		detailSource = matchedCode?.[1] ?? "provider_code";
	} else if (statusCategory) {
		category = statusCategory;
		detailSource = "http_status";
	} else if (
		unsupportedEvidence ||
		(phase === "stream" &&
			messageCategory === "transport" &&
			(!(value instanceof Error) || (value instanceof APIError && !sdkConnection)))
	) {
		category = "provider_error";
		detailSource = "none";
	} else {
		category = messageCategory ?? (kindHint === "malformed_event" ? "malformed_event" : "unknown");
		detailSource = messageCategory ? "message_category" : "none";
	}
	const hasEvidence = Boolean(
		unsupportedEvidence ||
			top.code ||
			(top.type !== "error" && top.type !== "response.failed" && top.type) ||
			top.message ||
			top.status ||
			nested.code ||
			nested.type ||
			nested.message ||
			nested.status,
	);
	const errorName = value instanceof Error ? value.name.toLowerCase() : "";
	const inferredTransport =
		kindHint !== "provider_event" &&
		!sdkAbort &&
		status === undefined &&
		providerCode === undefined &&
		!top.code &&
		!nested.code &&
		!nested.type &&
		!contextOverflow &&
		(phase === "request"
			? sdkConnection ||
				(messageCategory === undefined && (errorName.includes("connection") || errorName === "typeerror"))
			: sdkConnection ||
				transportCause ||
				(value instanceof Error &&
					/^terminated$/i.test(top.message ?? "") &&
					!unsupportedEvidence &&
					Object.keys(value).every((key) => ["name", "message", "stack", "cause"].includes(key))));
	if (inferredTransport) {
		category =
			value instanceof APIConnectionTimeoutError
				? "timeout"
				: messageCategory === "timeout"
					? "timeout"
					: "transport";
		detailSource = "none";
	}
	const kind =
		kindHint ??
		(status
			? "http"
			: inferredTransport || (phase === "request" && category === "transport")
				? "transport"
				: hasEvidence
					? "provider_event"
					: "malformed_event");
	if (kind === "malformed_event") {
		category = "malformed_event";
		detailSource = "none";
	} else if (kind === "provider_event" && category === "unknown" && !hasEvidence) {
		category = "malformed_event";
	}
	const diagnostic: ResponsesProviderFailureV1 = {
		schemaVersion: 1,
		layer: "openai_responses",
		phase,
		kind,
		category,
		retryDisposition: CATEGORY_DISPOSITIONS[category],
		detailSource,
	};
	if (status !== undefined) diagnostic.httpStatus = status;
	if (
		providerCode !== undefined &&
		(detailSource === "provider_code" ||
			detailSource === "provider_type" ||
			(detailSource === "message_category" && category === "context_overflow"))
	) {
		diagnostic.providerCode = providerCode;
	}
	const errorFields = unfamiliarFieldNames(recordOf(recordOf(value)?.error) ?? recordOf(value), [
		"code",
		"type",
		"message",
		"status",
		"param",
		"error",
		"headers",
		"requestID",
		"cause",
		"stack",
		"name",
		"id",
		"sequence_number",
	]);
	const causeMessage = boundedMessage(cause?.message);
	const providerMessage =
		top.message ??
		nested.message ??
		boundedMessage(recordOf(value)?.error) ??
		boundedMessage(recordOf(recordOf(value)?.error)?.detail) ??
		boundedMessage(recordOf(value)?.detail) ??
		(errorFields.length ? `Unrecognized error fields: ${errorFields.join(", ")}` : undefined) ??
		[top.code, nested.code].find((code) => code && code !== "error");
	const detail =
		kind === "malformed_event"
			? undefined
			: readableFailureDetail(
					causeMessage && causeMessage !== providerMessage
						? `${providerMessage ? `${providerMessage} — ` : ""}${causeMessage}`
						: providerMessage,
				);
	return { message: composeFailureMessage(category, detail), diagnostic };
}

export function normalizeResponsesFailure(value: unknown, phase: "request" | "stream"): SafeResponsesFailure {
	return makeFailure(value, phase);
}

function sdkRetryOrdinal(input: string | URL | Request, init: RequestInit | undefined, fallback: number): number {
	try {
		const headers = init?.headers ? new Headers(init.headers) : input instanceof Request ? input.headers : undefined;
		const value = headers?.get("x-stainless-retry-count");
		if (value && /^\d+$/.test(value)) {
			const ordinal = Number(value);
			if (Number.isSafeInteger(ordinal) && ordinal <= 65_535) return ordinal;
		}
	} catch {
		// Observation must not affect the request.
	}
	return Math.min(fallback, 65_535);
}

function transportCategory(value: unknown): "timeout" | "connection" | "other" {
	if (!(value instanceof Error)) return "other";
	const name = value.name.toLowerCase();
	if (name.includes("timeout") || name.includes("abort")) return "timeout";
	if (name === "typeerror" || name.includes("connection")) return "connection";
	return "other";
}

function isSdkRetryableStatus(status: number | undefined): boolean {
	return status === 408 || status === 409 || status === 429 || (status !== undefined && status >= 500);
}

const OPENAI_SDK_DEFAULT_MAX_RETRIES = 2;

// SCRAMJET-DIVERGENCE: Observe bounded SDK request attempts without changing fetch behavior or retry policy (#553).
export function createResponsesSdkRequestObserver(fetchImplementation: typeof fetch): ResponsesSdkRequestObserver {
	let observedAttemptCount = 0;
	let accepted = false;
	const firstAttempts: ResponsesSdkRetryAttempt[] = [];
	const latestAttempts: ResponsesSdkRetryAttempt[] = [];

	const record = (attempt: ResponsesSdkRetryAttempt): void => {
		observedAttemptCount = Math.min(observedAttemptCount + 1, 65_535);
		if (firstAttempts.length < 4) {
			firstAttempts.push(attempt);
			return;
		}
		latestAttempts.push(attempt);
		if (latestAttempts.length > 4) latestAttempts.shift();
	};
	const attempts = (): ResponsesSdkRetryAttempt[] => [...firstAttempts, ...latestAttempts];
	const diagnostic = (
		outcome: ResponsesSdkRetryV1["outcome"],
		reason: ResponsesSdkRetryReason,
	): ResponsesSdkRetryV1 => ({
		schemaVersion: 1,
		layer: "openai_sdk_request",
		outcome,
		reason,
		observedAttemptCount,
		attempts: attempts(),
		truncated: observedAttemptCount > 8,
	});

	return {
		fetch: async (input, init) => {
			const fallbackOrdinal = observedAttemptCount;
			let response: Response;
			try {
				response = await fetchImplementation(input, init);
			} catch (error) {
				try {
					record({
						ordinal: sdkRetryOrdinal(input, init, fallbackOrdinal),
						result: "transport",
						category: transportCategory(error),
					});
				} catch {
					// Observation must not replace the transport failure.
				}
				throw error;
			}
			try {
				const attempt: ResponsesSdkRetryAttempt = {
					ordinal: sdkRetryOrdinal(input, init, fallbackOrdinal),
					result: "response",
				};
				if (finiteStatus(response.status) !== undefined) attempt.status = response.status;
				record(attempt);
			} catch {
				// Observation must not replace or alter the response.
			}
			return response;
		},
		markAccepted: () => {
			accepted = true;
		},
		diagnosticForSuccess: () =>
			observedAttemptCount > 1 ? diagnostic("recovered", "accepted_after_retry") : undefined,
		diagnosticForFailure: (maxRetries) => {
			if (accepted) {
				return observedAttemptCount > 1
					? diagnostic("recovered", "accepted_after_retry")
					: diagnostic("not_attempted", "stream_already_accepted");
			}
			if (observedAttemptCount > 1) {
				const effectiveMaxRetries = maxRetries ?? OPENAI_SDK_DEFAULT_MAX_RETRIES;
				const reachedConfiguredLimit = observedAttemptCount >= Math.min(effectiveMaxRetries + 1, 65_535);
				return diagnostic(
					"exhausted",
					reachedConfiguredLimit ? "configured_limit_reached" : "terminal_after_retry",
				);
			}
			if (observedAttemptCount === 1) {
				if (maxRetries === 0) return diagnostic("not_attempted", "configured_zero");
				const [attempt] = attempts();
				if (attempt?.result === "response" && !isSdkRetryableStatus(attempt.status)) {
					return diagnostic("not_attempted", "non_retryable_request");
				}
			}
			return diagnostic("not_attempted", "insufficient_evidence");
		},
	};
}

export function appendResponsesSdkRetryDiagnostic(
	output: AssistantMessage,
	diagnostic: ResponsesSdkRetryV1 | undefined,
): void {
	if (!diagnostic) return;
	output.diagnostics = [
		...(output.diagnostics ?? []),
		{ type: "sdk_request_retry", timestamp: Date.now(), details: { ...diagnostic } },
	];
}

export function appendResponsesFailureDiagnostics(
	output: AssistantMessage,
	failure: SafeResponsesFailure,
	sdkRetry?: ResponsesSdkRetryV1,
): void {
	output.errorMessage = failure.message;
	output.diagnostics = [
		...(output.diagnostics ?? []),
		{ type: "provider_failure", timestamp: Date.now(), details: { ...failure.diagnostic } },
		...(sdkRetry ? [{ type: "sdk_request_retry", timestamp: Date.now(), details: { ...sdkRetry } }] : []),
		{ type: "gateway_observability", timestamp: Date.now(), details: { ...GATEWAY_OBSERVABILITY } },
	];
}

function isProviderFailureDetails(value: unknown): value is ResponsesProviderFailureV1 {
	const details = recordOf(value);
	if (!details || Object.keys(details).some((key) => !FAILURE_DETAIL_KEYS.has(key))) return false;
	const category = details.category;
	const kind = details.kind;
	const source = details.detailSource;
	const status = details.httpStatus;
	const providerCode = details.providerCode;
	if (
		details.schemaVersion !== 1 ||
		details.layer !== "openai_responses" ||
		(details.phase !== "request" && details.phase !== "stream") ||
		typeof kind !== "string" ||
		!["http", "provider_event", "transport", "malformed_event"].includes(kind) ||
		typeof category !== "string" ||
		!(category in CATEGORY_DISPOSITIONS) ||
		details.retryDisposition !== CATEGORY_DISPOSITIONS[category as ResponsesFailureCategory] ||
		typeof source !== "string" ||
		!["provider_code", "provider_type", "http_status", "message_category", "none"].includes(source) ||
		(status !== undefined && finiteStatus(status) === undefined) ||
		(providerCode !== undefined &&
			(typeof providerCode !== "string" || allowlistedProviderCode(providerCode) === undefined))
	) {
		return false;
	}
	if (kind === "malformed_event") {
		return category === "malformed_event" && source === "none" && status === undefined && providerCode === undefined;
	}
	if (kind === "transport") {
		if (status !== undefined || providerCode !== undefined) return false;
		if (source === "none") return category === "transport" || category === "timeout";
		return details.phase === "request" && source === "message_category" && category === "transport";
	}
	if (kind === "http" && status === undefined) return false;
	if (kind === "provider_event" && details.phase === "request" && status !== undefined) return false;
	if (kind === "provider_event" && details.phase === "request" && category === "transport") return false;
	if (source === "provider_code" || source === "provider_type") {
		return (
			typeof providerCode === "string" &&
			PROVIDER_CODE_CATEGORIES[providerCode as ResponsesProviderCode] === category
		);
	}
	if (source === "http_status") {
		return providerCode === undefined && categoryFromStatus(status as number | undefined) === category;
	}
	if (source === "message_category") {
		if (category === "provider_error" || category === "malformed_event" || category === "unknown") return false;
		if (
			providerCode !== undefined &&
			(category !== "context_overflow" || providerCode === "context_length_exceeded")
		) {
			return false;
		}
		const statusCategory = categoryFromStatus(status as number | undefined);
		return statusCategory === undefined || category === "context_overflow";
	}
	if (providerCode !== undefined) return false;
	if (kind === "http") {
		const statusCategory = categoryFromStatus(status as number);
		return (
			(category === "unknown" && statusCategory === undefined) ||
			(category === "provider_error" &&
				(statusCategory === undefined || CATEGORY_DISPOSITIONS[statusCategory] === "transient"))
		);
	}
	if (status !== undefined) {
		return (
			kind === "provider_event" &&
			(category === "provider_error" || category === "unknown") &&
			categoryFromStatus(status as number) === undefined
		);
	}
	return (
		kind === "provider_event" &&
		(category === "provider_error" || category === "unknown" || category === "malformed_event")
	);
}

export function validateResponsesProviderFailure(diagnostics: unknown): ResponsesProviderFailureValidation {
	if (diagnostics === undefined) return { status: "absent" };
	if (!Array.isArray(diagnostics)) return { status: "malformed" };
	const matches: Record<string, unknown>[] = [];
	for (const diagnostic of diagnostics) {
		const candidate = recordOf(diagnostic);
		if (candidate?.type === "provider_failure") matches.push(candidate);
	}
	if (matches.length === 0) return { status: "absent" };
	if (matches.length > 1) return { status: "duplicate" };
	const details = matches[0].details;
	if (!isProviderFailureDetails(details)) return { status: "malformed" };
	return { status: "valid", category: details.category, retryDisposition: details.retryDisposition };
}

function providerEventFailure(value: unknown, kindHint?: ResponsesFailureKind): SafeResponsesFailureError {
	return new SafeResponsesFailureError(makeFailure(value, "stream", kindHint));
}

// =============================================================================
// Message conversion
// =============================================================================

export function convertResponsesMessages<TApi extends Api>(
	model: Model<TApi>,
	context: Context,
	allowedToolCallProviders: ReadonlySet<string>,
	options?: ConvertResponsesMessagesOptions,
): ResponseInput {
	const messages: ResponseInput = [];

	const normalizeIdPart = (part: string): string => {
		const sanitized = part.replace(/[^a-zA-Z0-9_-]/g, "_");
		const normalized = sanitized.length > 64 ? sanitized.slice(0, 64) : sanitized;
		return normalized.replace(/_+$/, "");
	};

	const buildForeignResponsesItemId = (itemId: string): string => {
		const normalized = `fc_${shortHash(itemId)}`;
		return normalized.length > 64 ? normalized.slice(0, 64) : normalized;
	};

	const normalizeToolCallId = (id: string, _targetModel: Model<TApi>, source: AssistantMessage): string => {
		if (!allowedToolCallProviders.has(model.provider)) return normalizeIdPart(id);
		if (!id.includes("|")) return normalizeIdPart(id);
		const [callId, itemId] = id.split("|");
		const normalizedCallId = normalizeIdPart(callId);
		const isForeignToolCall = source.provider !== model.provider || source.api !== model.api;
		let normalizedItemId = isForeignToolCall ? buildForeignResponsesItemId(itemId) : normalizeIdPart(itemId);
		// OpenAI Responses API requires item id to start with "fc"
		if (!normalizedItemId.startsWith("fc_")) {
			normalizedItemId = normalizeIdPart(`fc_${normalizedItemId}`);
		}
		return `${normalizedCallId}|${normalizedItemId}`;
	};

	const transformedMessages = transformMessages(context.messages, model, normalizeToolCallId);

	const includeSystemPrompt = options?.includeSystemPrompt ?? true;
	const systemPrompt = flattenSystemPrompt(context.systemPrompt);
	if (includeSystemPrompt && systemPrompt) {
		const role = model.reasoning ? "developer" : "system";
		messages.push({
			role,
			content: sanitizeSurrogates(systemPrompt),
		});
	}

	let msgIndex = 0;
	for (const msg of transformedMessages) {
		if (msg.role === "user") {
			if (typeof msg.content === "string") {
				messages.push({
					role: "user",
					content: [{ type: "input_text", text: sanitizeSurrogates(msg.content) }],
				});
			} else {
				const content: ResponseInputContent[] = msg.content.map((item): ResponseInputContent => {
					if (item.type === "text") {
						return {
							type: "input_text",
							text: sanitizeSurrogates(item.text),
						} satisfies ResponseInputText;
					}
					return {
						type: "input_image",
						detail: "auto",
						image_url: `data:${item.mimeType};base64,${item.data}`,
					} satisfies ResponseInputImage;
				});
				if (content.length === 0) continue;
				messages.push({
					role: "user",
					content,
				});
			}
		} else if (msg.role === "assistant") {
			const output: ResponseInput = [];
			const assistantMsg = msg as AssistantMessage;
			const isDifferentModel =
				assistantMsg.model !== model.id &&
				assistantMsg.provider === model.provider &&
				assistantMsg.api === model.api;

			for (const block of msg.content) {
				if (block.type === "thinking") {
					if (block.thinkingSignature) {
						const reasoningItem = JSON.parse(block.thinkingSignature) as ResponseReasoningItem;
						output.push(reasoningItem);
					}
				} else if (block.type === "text") {
					const textBlock = block as TextContent;
					const parsedSignature = parseTextSignature(textBlock.textSignature);
					// OpenAI requires id to be max 64 characters
					let msgId = parsedSignature?.id;
					if (!msgId) {
						msgId = `msg_${msgIndex}`;
					} else if (msgId.length > 64) {
						msgId = `msg_${shortHash(msgId)}`;
					}
					output.push({
						type: "message",
						role: "assistant",
						content: [{ type: "output_text", text: sanitizeSurrogates(textBlock.text), annotations: [] }],
						status: "completed",
						id: msgId,
						phase: parsedSignature?.phase,
					} satisfies ResponseOutputMessage);
				} else if (block.type === "toolCall") {
					const toolCall = block as ToolCall;
					const [callId, itemIdRaw] = toolCall.id.split("|");
					let itemId: string | undefined = itemIdRaw;

					// For different-model messages, set id to undefined to avoid pairing validation.
					// OpenAI tracks which fc_xxx IDs were paired with rs_xxx reasoning items.
					// By omitting the id, we avoid triggering that validation (like cross-provider does).
					if (isDifferentModel && itemId?.startsWith("fc_")) {
						itemId = undefined;
					}

					output.push({
						type: "function_call",
						id: itemId,
						call_id: callId,
						name: toolCall.name,
						arguments: JSON.stringify(toolCall.arguments),
					});
				}
			}
			if (output.length === 0) continue;
			messages.push(...output);
		} else if (msg.role === "toolResult") {
			const textResult = msg.content
				.filter((c): c is TextContent => c.type === "text")
				.map((c) => c.text)
				.join("\n");
			const hasImages = msg.content.some((c): c is ImageContent => c.type === "image");
			const hasText = textResult.length > 0;
			const [callId] = msg.toolCallId.split("|");

			let output: string | ResponseFunctionCallOutputItemList;
			if (hasImages && model.input.includes("image")) {
				const contentParts: ResponseFunctionCallOutputItemList = [];

				if (hasText) {
					contentParts.push({
						type: "input_text",
						text: sanitizeSurrogates(textResult),
					});
				}

				for (const block of msg.content) {
					if (block.type === "image") {
						contentParts.push({
							type: "input_image",
							detail: "auto",
							image_url: `data:${block.mimeType};base64,${block.data}`,
						});
					}
				}

				output = contentParts;
			} else {
				output = sanitizeSurrogates(hasText ? textResult : "(see attached image)");
			}

			messages.push({
				type: "function_call_output",
				call_id: callId,
				output,
			});
		}
		msgIndex++;
	}

	return messages;
}

// =============================================================================
// Tool conversion
// =============================================================================

export function convertResponsesTools(tools: Tool[], options?: ConvertResponsesToolsOptions): OpenAITool[] {
	const strict = options?.strict === undefined ? false : options.strict;
	return tools.map((tool) => ({
		type: "function",
		name: tool.name,
		description: tool.description,
		parameters: tool.parameters as any, // TypeBox already generates JSON Schema
		strict,
	}));
}

// =============================================================================
// Stream processing
// =============================================================================

export async function processResponsesStream<TApi extends Api>(
	openaiStream: AsyncIterable<ResponseStreamEvent>,
	output: AssistantMessage,
	stream: AssistantMessageEventStream,
	model: Model<TApi>,
	options?: OpenAIResponsesStreamOptions,
): Promise<void> {
	let currentItem: ResponseReasoningItem | ResponseOutputMessage | ResponseFunctionToolCall | null = null;
	let currentBlock: ThinkingContent | TextContent | (ToolCall & { partialJson: string }) | null = null;
	const blocks = output.content;
	const blockIndex = () => blocks.length - 1;

	for await (const event of openaiStream) {
		if (event.type === "response.created") {
			output.responseId = event.response.id;
		} else if (event.type === "response.output_item.added") {
			const item = event.item;
			if (item.type === "reasoning") {
				currentItem = item;
				currentBlock = { type: "thinking", thinking: "" };
				output.content.push(currentBlock);
				stream.push({ type: "thinking_start", contentIndex: blockIndex(), partial: output });
			} else if (item.type === "message") {
				currentItem = item;
				currentBlock = { type: "text", text: "" };
				output.content.push(currentBlock);
				stream.push({ type: "text_start", contentIndex: blockIndex(), partial: output });
			} else if (item.type === "function_call") {
				currentItem = item;
				currentBlock = {
					type: "toolCall",
					id: `${item.call_id}|${item.id}`,
					name: item.name,
					arguments: {},
					partialJson: item.arguments || "",
				};
				output.content.push(currentBlock);
				stream.push({ type: "toolcall_start", contentIndex: blockIndex(), partial: output });
			}
		} else if (event.type === "response.reasoning_summary_part.added") {
			if (currentItem && currentItem.type === "reasoning") {
				currentItem.summary = currentItem.summary || [];
				currentItem.summary.push(event.part);
			}
		} else if (event.type === "response.reasoning_summary_text.delta") {
			if (currentItem?.type === "reasoning" && currentBlock?.type === "thinking") {
				currentItem.summary = currentItem.summary || [];
				const lastPart = currentItem.summary[currentItem.summary.length - 1];
				if (lastPart) {
					currentBlock.thinking += event.delta;
					lastPart.text += event.delta;
					stream.push({
						type: "thinking_delta",
						contentIndex: blockIndex(),
						delta: event.delta,
						partial: output,
					});
				}
			}
		} else if (event.type === "response.reasoning_summary_part.done") {
			if (currentItem?.type === "reasoning" && currentBlock?.type === "thinking") {
				currentItem.summary = currentItem.summary || [];
				const lastPart = currentItem.summary[currentItem.summary.length - 1];
				if (lastPart) {
					currentBlock.thinking += "\n\n";
					lastPart.text += "\n\n";
					stream.push({
						type: "thinking_delta",
						contentIndex: blockIndex(),
						delta: "\n\n",
						partial: output,
					});
				}
			}
		} else if (event.type === "response.reasoning_text.delta") {
			if (currentItem?.type === "reasoning" && currentBlock?.type === "thinking") {
				currentBlock.thinking += event.delta;
				stream.push({
					type: "thinking_delta",
					contentIndex: blockIndex(),
					delta: event.delta,
					partial: output,
				});
			}
		} else if (event.type === "response.content_part.added") {
			if (currentItem?.type === "message") {
				currentItem.content = currentItem.content || [];
				// Filter out ReasoningText, only accept output_text and refusal
				if (event.part.type === "output_text" || event.part.type === "refusal") {
					currentItem.content.push(event.part);
				}
			}
		} else if (event.type === "response.output_text.delta") {
			if (currentItem?.type === "message" && currentBlock?.type === "text") {
				if (!currentItem.content || currentItem.content.length === 0) {
					continue;
				}
				const lastPart = currentItem.content[currentItem.content.length - 1];
				if (lastPart?.type === "output_text") {
					currentBlock.text += event.delta;
					lastPart.text += event.delta;
					stream.push({
						type: "text_delta",
						contentIndex: blockIndex(),
						delta: event.delta,
						partial: output,
					});
				}
			}
		} else if (event.type === "response.refusal.delta") {
			if (currentItem?.type === "message" && currentBlock?.type === "text") {
				if (!currentItem.content || currentItem.content.length === 0) {
					continue;
				}
				const lastPart = currentItem.content[currentItem.content.length - 1];
				if (lastPart?.type === "refusal") {
					currentBlock.text += event.delta;
					lastPart.refusal += event.delta;
					stream.push({
						type: "text_delta",
						contentIndex: blockIndex(),
						delta: event.delta,
						partial: output,
					});
				}
			}
		} else if (event.type === "response.function_call_arguments.delta") {
			if (currentItem?.type === "function_call" && currentBlock?.type === "toolCall") {
				currentBlock.partialJson += event.delta;
				currentBlock.arguments = parseStreamingJson(currentBlock.partialJson);
				stream.push({
					type: "toolcall_delta",
					contentIndex: blockIndex(),
					delta: event.delta,
					partial: output,
				});
			}
		} else if (event.type === "response.function_call_arguments.done") {
			if (currentItem?.type === "function_call" && currentBlock?.type === "toolCall") {
				const previousPartialJson = currentBlock.partialJson;
				currentBlock.partialJson = event.arguments;
				currentBlock.arguments = parseStreamingJson(currentBlock.partialJson);

				if (event.arguments.startsWith(previousPartialJson)) {
					const delta = event.arguments.slice(previousPartialJson.length);
					if (delta.length > 0) {
						stream.push({
							type: "toolcall_delta",
							contentIndex: blockIndex(),
							delta,
							partial: output,
						});
					}
				}
			}
		} else if (event.type === "response.output_item.done") {
			const item = event.item;

			if (item.type === "reasoning" && currentBlock?.type === "thinking") {
				const summaryText = item.summary?.map((s) => s.text).join("\n\n") || "";
				const contentText = item.content?.map((c) => c.text).join("\n\n") || "";
				currentBlock.thinking = summaryText || contentText || currentBlock.thinking;
				currentBlock.thinkingSignature = JSON.stringify(item);
				stream.push({
					type: "thinking_end",
					contentIndex: blockIndex(),
					content: currentBlock.thinking,
					partial: output,
				});
				currentBlock = null;
			} else if (item.type === "message" && currentBlock?.type === "text") {
				currentBlock.text = item.content.map((c) => (c.type === "output_text" ? c.text : c.refusal)).join("");
				currentBlock.textSignature = encodeTextSignatureV1(item.id, item.phase ?? undefined);
				stream.push({
					type: "text_end",
					contentIndex: blockIndex(),
					content: currentBlock.text,
					partial: output,
				});
				currentBlock = null;
			} else if (item.type === "function_call") {
				const args =
					currentBlock?.type === "toolCall" && currentBlock.partialJson
						? parseStreamingJson(currentBlock.partialJson)
						: parseStreamingJson(item.arguments || "{}");

				let toolCall: ToolCall;
				if (currentBlock?.type === "toolCall") {
					// Finalize in-place and strip the scratch buffer so replay only
					// carries parsed arguments.
					currentBlock.arguments = args;
					delete (currentBlock as { partialJson?: string }).partialJson;
					toolCall = currentBlock;
				} else {
					toolCall = {
						type: "toolCall",
						id: `${item.call_id}|${item.id}`,
						name: item.name,
						arguments: args,
					};
				}

				currentBlock = null;
				stream.push({ type: "toolcall_end", contentIndex: blockIndex(), toolCall, partial: output });
			}
		} else if (event.type === "response.completed") {
			const response = event.response;
			if (response?.id) {
				output.responseId = response.id;
			}
			if (response?.usage) {
				const cachedTokens = response.usage.input_tokens_details?.cached_tokens || 0;
				output.usage = {
					// OpenAI includes cached tokens in input_tokens, so subtract to get non-cached input
					input: (response.usage.input_tokens || 0) - cachedTokens,
					output: response.usage.output_tokens || 0,
					cacheRead: cachedTokens,
					cacheWrite: 0,
					totalTokens: response.usage.total_tokens || 0,
					cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
				};
			}
			calculateCost(model, output.usage);
			if (options?.applyServiceTierPricing) {
				const serviceTier = options.resolveServiceTier
					? options.resolveServiceTier(response?.service_tier, options.serviceTier)
					: (response?.service_tier ?? options.serviceTier);
				options.applyServiceTierPricing(output.usage, serviceTier);
			}
			// Map status to stop reason
			output.stopReason = mapStopReason(response?.status);
			if (output.content.some((b) => b.type === "toolCall") && output.stopReason === "stop") {
				output.stopReason = "toolUse";
			}
		} else if (event.type === "error") {
			throw providerEventFailure(event);
		} else if (event.type === "response.failed") {
			const error = event.response?.error;
			const details = event.response?.incomplete_details;
			if (error) throw providerEventFailure(error, "provider_event");
			if (details?.reason) throw providerEventFailure({ message: details.reason }, "provider_event");
			const response = recordOf(event.response);
			const fields = unfamiliarFieldNames(response, ["id", "status", "output", "usage"]);
			const rich =
				response && Object.keys(response).some((key) => !["id", "status", "output", "usage"].includes(key));
			throw providerEventFailure(
				rich
					? {
							type: "response.failed",
							reason: "unsupported_details",
							message: `Unrecognized response fields${fields.length ? `: ${fields.join(", ")}` : ""}`,
						}
					: event,
				rich ? "provider_event" : "malformed_event",
			);
		}
	}
}

function mapStopReason(status: OpenAI.Responses.ResponseStatus | undefined): StopReason {
	if (!status) return "stop";
	switch (status) {
		case "completed":
			return "stop";
		case "incomplete":
			return "length";
		case "failed":
		case "cancelled":
			return "error";
		// These two are wonky ...
		case "in_progress":
		case "queued":
			return "stop";
		default: {
			const _exhaustive: never = status;
			throw new Error(`Unhandled stop reason: ${_exhaustive}`);
		}
	}
}
