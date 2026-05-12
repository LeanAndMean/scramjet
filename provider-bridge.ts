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

export function registerProviderBridge(pi: ExtensionAPI): void {
	const config = resolveProviderBridgeConfig(process.env);
	if (config === null) return;
	pi.registerProvider("anthropic", config);
}
