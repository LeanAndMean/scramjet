import { describe, expect, it } from "vitest";
import {
	normalizeResponsesFailure,
	validateResponsesProviderFailure,
} from "../src/providers/openai-responses-shared.js";
import type { AssistantMessage } from "../src/types.js";
import {
	appendObservedFailure,
	blocksFailureRecovery,
	failureFromProviderError,
	inspectFailureEvidence,
	invokeProviderCallback,
	validateRequestFailure,
} from "../src/utils/failure-evidence.js";
import { isContextOverflow } from "../src/utils/overflow.js";

const diagnostic = (details: unknown, type = "request_failure") => ({ type, timestamp: 0, details });
const message = (): AssistantMessage => ({
	role: "assistant",
	content: [],
	api: "openai-completions",
	provider: "openai",
	model: "test",
	stopReason: "error",
	errorMessage: "maximum context length exceeded",
	timestamp: 1,
	usage: {
		input: 0,
		output: 0,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens: 0,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	},
});

describe("persisted failure evidence", () => {
	it.each(["ECONNRESET", "EPIPE", "ETIMEDOUT", "UND_ERR_SOCKET"])("retains native %s transport provenance", (code) => {
		const result = message();
		appendObservedFailure(result, { code, message: "neutral" });
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ transient: true });
		const denied = message();
		appendObservedFailure(denied, { status: 401, code, message: "neutral" });
		expect(inspectFailureEvidence(denied.diagnostics)).toMatchObject({
			category: "authentication",
			transient: false,
		});
	});
	it.each([408, 429, 500, 501, 502, 503, 504, 529, 599])(
		"retains transient HTTP %s without relying on prose",
		(status) => {
			const result = message();
			appendObservedFailure(result, { status, message: "neutral" });
			expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({
				status: "valid",
				transient: true,
				source: "provider",
			});
			expect(isContextOverflow(result)).toBe(false);
		},
	);
	it.each([400, 401, 403, 404, 409, 422])("does not retry HTTP %s because of conflicting transient code", (status) => {
		const result = message();
		appendObservedFailure(result, { status, error: { code: "rate_limit_exceeded" } });
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", transient: false });
	});
	it("preserves recognized overflow inside generic invalid request", () => {
		const result = message();
		appendObservedFailure(result, {
			status: 400,
			error: { type: "invalid_request_error", message: "prompt is too long: 250000 tokens" },
		});
		expect(isContextOverflow(result)).toBe(true);
	});
	it.each([401, 403, 404])("Responses HTTP %s vetoes conflicting overflow", (status) => {
		const produced = normalizeResponsesFailure(
			{ status, error: { code: "context_length_exceeded" } },
			"request",
		).diagnostic;
		expect(validateResponsesProviderFailure([diagnostic(produced, "provider_failure")])).toMatchObject({
			status: "valid",
			category: status === 401 ? "authentication" : status === 403 ? "permission" : "not_found",
		});
		const contradictory = {
			...produced,
			category: "context_overflow",
			retryDisposition: "non_transient",
			detailSource: "provider_code",
			providerCode: "context_length_exceeded",
		};
		expect(validateResponsesProviderFailure([diagnostic(contradictory, "provider_failure")])).toEqual({
			status: "malformed",
		});
	});
	it("does not infer transport from a generic request-phase TypeError", () => {
		expect(
			normalizeResponsesFailure(new TypeError("Do not know how to serialize a BigInt"), "request").diagnostic
				.category,
		).not.toBe("transport");
	});
	it.each(["unknown", "provider_error", "malformed_event"])(
		"blocks failure-triggered recovery for explicit %s",
		(category) => {
			const details =
				category === "malformed_event"
					? { schemaVersion: 1, kind: "stream", reason: category }
					: category === "provider_error"
						? {
								schemaVersion: 1,
								layer: "openai_responses",
								phase: "stream",
								kind: "provider_event",
								category,
								retryDisposition: "unknown",
								detailSource: "none",
							}
						: { schemaVersion: 1, kind: "provider", category };
			expect(
				inspectFailureEvidence([
					diagnostic(details, category === "provider_error" ? "provider_failure" : "request_failure"),
				]),
			).toMatchObject({ status: "valid", category });
			expect(
				blocksFailureRecovery({
					...message(),
					diagnostics: [
						diagnostic(details, category === "provider_error" ? "provider_failure" : "request_failure"),
					],
				}),
			).toBe(true);
		},
	);
	it("preserves absent-evidence legacy overflow", () => expect(isContextOverflow(message())).toBe(true));
	it.each([
		{ status: 429, error: { code: "rate_limit_exceeded", type: "insufficient_quota" } },
		{ status: 429, error: { code: "insufficient_quota", type: "rate_limit_exceeded" } },
		{ status: 429, code: "insufficient_quota", error: { type: "rate_limit_exceeded" } },
		{ status: 429, cause: { code: "insufficient_quota" }, error: { code: "rate_limit_exceeded" } },
	])("closes conflicting recovery-relevant semantic fields", (error) => {
		expect(failureFromProviderError(error)).toEqual({ schemaVersion: 1, kind: "provider", category: "unknown" });
	});
	it.each([
		[{ status: 429, error: { code: "rate_limit_exceeded", type: "rate_limit_error" } }, "rate_limit"],
		[{ status: 429, code: "wrapper", error: { type: "rate_limit_error" } }, "rate_limit"],
		[{ status: 429, error: { code: "novel", type: "wrapper" } }, "rate_limit"],
		[{ status: 503, error: { code: "novel" } }, "server"],
		[{ error: { code: "insufficient_quota", type: "billing_hard_limit_reached" } }, "quota_exhausted"],
	] as const)("retains compatible semantics and observed status", (error, category) => {
		const result = message();
		appendObservedFailure(result, error);
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ status: "valid", category });
	});
	it("keeps quota separate from HTTP rate limiting", () => {
		expect(failureFromProviderError({ status: 429, error: { code: "insufficient_quota" } })).toMatchObject({
			category: "quota_exhausted",
		});
		expect(
			failureFromProviderError({ status: 429, message: JSON.stringify({ error: { code: "usage_limit_reached" } }) }),
		).toMatchObject({ category: "quota_exhausted" });
	});
	it.each([
		{ schemaVersion: 2, kind: "http", status: 503, reason: "status" },
		{ schemaVersion: 1, kind: "http", status: "503", reason: "status" },
		{ schemaVersion: 1, kind: "http", status: 200, reason: "status" },
		{ schemaVersion: 1, kind: "http", status: 503, reason: "status", private: true },
		{ schemaVersion: 1, kind: "stream", reason: "novel" },
	])("rejects malformed recognized evidence", (details) => {
		const result = { ...message(), diagnostics: [diagnostic(details)] };
		expect(validateRequestFailure(result.diagnostics)).toEqual({ status: "malformed" });
		expect(isContextOverflow(result)).toBe(false);
	});
	it.each([401, 403, 404])("rejects contradictory semantic reasons for terminal HTTP %s", (status) => {
		for (const reason of [
			"quota_exhausted",
			"context_overflow",
			"authentication",
			"permission",
			"invalid_request",
			"content_rejection",
			"unknown",
		]) {
			const result = { ...message(), diagnostics: [diagnostic({ schemaVersion: 1, kind: "http", status, reason })] };
			const consistent =
				(status === 401 && reason === "authentication") || (status === 403 && reason === "permission");
			expect(validateRequestFailure(result.diagnostics).status).toBe(consistent ? "valid" : "malformed");
			expect(isContextOverflow(result)).toBe(false);
		}
	});
	it.each([
		[400, "context_overflow"],
		[413, "context_overflow"],
		[429, "quota_exhausted"],
		[503, "quota_exhausted"],
	])("retains legitimate HTTP %s / %s evidence", (status, reason) => {
		const result = { ...message(), diagnostics: [diagnostic({ schemaVersion: 1, kind: "http", status, reason })] };
		expect(validateRequestFailure(result.diagnostics)).toMatchObject({ status: "valid", category: reason });
		expect(isContextOverflow(result)).toBe(reason === "context_overflow");
	});
	it("rejects duplicated and conflicting authorities", () => {
		const d = diagnostic({ schemaVersion: 1, kind: "http", status: 503, reason: "status" });
		expect(inspectFailureEvidence([d, d])).toEqual({ status: "duplicate" });
		expect(inspectFailureEvidence([d, diagnostic({}, "provider_failure")])).toEqual({ status: "conflicting" });
	});
	it("requires a valid transient failure for delay suppression", () => {
		const d = diagnostic({ schemaVersion: 1, kind: "http", status: 429, reason: "status" });
		const suppression = diagnostic(
			{ schemaVersion: 1, reason: "server_delay_exceeds_limit", requestedDelayMs: 120000, maxDelayMs: 60000 },
			"retry_suppression",
		);
		expect(inspectFailureEvidence([suppression])).toEqual({ status: "malformed" });
		expect(inspectFailureEvidence([d, suppression])).toMatchObject({
			suppression: "server_delay_exceeds_limit",
			category: "rate_limit",
		});
		expect(blocksFailureRecovery({ ...message(), diagnostics: [d, suppression] })).toBe(true);
		expect(inspectFailureEvidence([d, suppression, suppression])).toEqual({ status: "duplicate" });
	});
	it("does not persist callback secrets or infer provider failure from callback text", async () => {
		const result = message();
		try {
			await invokeProviderCallback("onPayload", () => {
				throw new Error("private 429 maximum context length sentinel");
			});
		} catch (error) {
			appendObservedFailure(result, error);
			result.errorMessage = (error as Error).message;
		}
		expect(inspectFailureEvidence(result.diagnostics)).toMatchObject({ source: "callback" });
		expect(isContextOverflow(result)).toBe(false);
		expect(blocksFailureRecovery(result)).toBe(true);
		expect(JSON.stringify(result)).not.toContain("sentinel");
	});
});
