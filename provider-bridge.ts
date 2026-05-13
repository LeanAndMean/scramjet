/**
 * Provider bridge - auto-detect proxy config from environment.
 *
 * When ANTHROPIC_BASE_URL points at a non-stock host (e.g. a tux or Foundry
 * proxy), redirect Pi's Anthropic provider through it. Pi hard-codes
 * baseUrl: "https://api.anthropic.com" on each Anthropic model entry, so the
 * SDK's normal env-var fallback never engages. This bridge calls
 * pi.registerProvider("anthropic", ...) at load time to override the endpoint.
 *
 * Detection follows the Claude Code env-var convention so a single
 * `source ~/.claudecode_tux` configures both Claude Code and Pi.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

interface ProviderBridgeConfig {
	baseUrl: string;
	apiKey?: string;
}

export function resolveProviderBridgeConfig(env: NodeJS.ProcessEnv): ProviderBridgeConfig | null {
	if (env.SCRAMJET_PROVIDER_BRIDGE === "0") return null;

	const baseUrl = env.ANTHROPIC_BASE_URL;
	if (!baseUrl) return null;

	// Throws on malformed URL — intentional fail-fast so a misconfigured
	// ANTHROPIC_BASE_URL surfaces loudly at startup rather than silently
	// disabling the bridge. The empty/dot hostname check catches degenerate
	// inputs like `https://.` that parse successfully but have no real host.
	const parsed = new URL(baseUrl);
	if (!parsed.hostname || parsed.hostname === ".") {
		throw new TypeError(`ANTHROPIC_BASE_URL has no valid hostname: "${baseUrl}"`);
	}
	if (parsed.hostname === "api.anthropic.com") return null;

	// Use || (not ??) so an explicit empty string in either var falls through
	// to the other. Pi rejects apiKey: "", so empty strings must collapse to
	// undefined and the field must be absent (not explicitly undefined).
	const rawKey = env.ANTHROPIC_AUTH_TOKEN || env.ANTHROPIC_API_KEY;
	const apiKey = rawKey || undefined;

	return apiKey !== undefined ? { baseUrl, apiKey } : { baseUrl };
}

/**
 * Strip `tools[].eager_input_streaming` from an Anthropic-shape request
 * payload. Foundry's Anthropic-API gateway rejects this field with
 * `INVALID_ARGUMENT: unrecognizedProperty=eager_input_streaming`. The
 * equivalent per-model knob (`compat.supportsEagerToolInputStreaming: false`)
 * isn't reachable from `pi.registerProvider`, so the bridge instead removes
 * the field in-flight when active. Non-Anthropic payloads don't carry the
 * field, so the strip is a no-op for them.
 *
 * Returns the mutated payload when something was stripped, `undefined`
 * otherwise — matching Pi's contract that `undefined` means "no change".
 */
export function stripEagerInputStreaming(payload: unknown): unknown {
	if (!payload || typeof payload !== "object") return undefined;
	const tools = (payload as { tools?: unknown }).tools;
	if (!Array.isArray(tools)) return undefined;

	let modified = false;
	for (const tool of tools) {
		if (tool && typeof tool === "object" && "eager_input_streaming" in tool) {
			delete (tool as Record<string, unknown>).eager_input_streaming;
			modified = true;
		}
	}
	return modified ? payload : undefined;
}

export function registerProviderBridge(pi: ExtensionAPI): void {
	const config = resolveProviderBridgeConfig(process.env);
	if (config === null) return;
	pi.registerProvider("anthropic", config);
	pi.on("before_provider_request", async (event) => stripEagerInputStreaming(event.payload));
}
