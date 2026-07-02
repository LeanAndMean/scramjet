// SCRAMJET-DIVERGENCE: shared Anthropic model pattern predicates (issue 245)
// Single source of truth for model family detection across anthropic.ts, amazon-bedrock.ts, and generate-models.ts.

/** Canonical dash-separated model ID substrings for models using adaptive thinking. */
export const ADAPTIVE_THINKING_PATTERNS = [
	"opus-4-6",
	"opus-4-7",
	"opus-4-8",
	"fable-5",
	"sonnet-4-6",
	"sonnet-5",
] as const;

/** Canonical dash-separated model ID substrings for models that reject temperature. */
export const TEMPERATURE_UNSUPPORTED_PATTERNS = ["opus-4-7", "opus-4-8"] as const;

/** Canonical dash-separated model ID substrings for models supporting native xhigh effort. */
export const NATIVE_XHIGH_EFFORT_PATTERNS = ["opus-4-7", "opus-4-8", "fable-5"] as const;

/** Normalize a model identifier for pattern matching: lowercase, dots/spaces/underscores → dashes. */
export function normalizeForPatternMatch(value: string): string {
	return value.toLowerCase().replace(/[\s_.:]+/g, "-");
}
