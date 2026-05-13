/**
 * Provider bridge - auto-detect proxy config from environment.
 *
 * When ANTHROPIC_BASE_URL points at a non-stock host (e.g. a tux or Foundry
 * proxy), redirect Pi's Anthropic provider through it. Pi hard-codes
 * baseUrl: "https://api.anthropic.com" on each Anthropic model entry, so the
 * SDK's normal env-var fallback never engages. This bridge calls
 * pi.registerProvider("anthropic", ...) at load time to override the endpoint.
 *
 * Also installs a `before_provider_request` hook that strips
 * `tools[].eager_input_streaming` so Foundry's gateway accepts the payload.
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

	// Reject URLs with an empty authority (e.g. `https:///path`) before
	// `new URL` consumes the path segment as the hostname.
	if (/^[a-z][a-z0-9+.-]*:\/\/\//i.test(baseUrl)) {
		throw new TypeError(`ANTHROPIC_BASE_URL has no authority: "${baseUrl}"`);
	}

	// Throws on malformed URL; `registerProviderBridge` catches and logs,
	// keeping this resolver pure while preserving the rest of the extension.
	// The all-dots hostname check catches degenerate inputs like `https://.`
	// and `https://..` that parse successfully but have no real host.
	const parsed = new URL(baseUrl);
	if (!parsed.hostname || parsed.hostname.replace(/\./g, "") === "") {
		throw new TypeError(`ANTHROPIC_BASE_URL has no valid hostname: "${baseUrl}"`);
	}

	// Strip a single trailing dot (FQDN-style) before comparing. Without this,
	// `https://api.anthropic.com.` would activate the bridge and route
	// Anthropic's own host through pi.registerProvider.
	const hostname = parsed.hostname.endsWith(".") ? parsed.hostname.slice(0, -1) : parsed.hostname;
	if (hostname === "api.anthropic.com") return null;

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
 * the field in-flight when active.
 *
 * The `before_provider_request` hook fires for ALL provider payloads (Pi's
 * `onPayload` does not pass a provider discriminator), so this function is
 * duck-typed for the Anthropic shape: payloads that don't carry a `tools[]`
 * array fall through and return `undefined` (no change) without touching
 * non-Anthropic providers.
 *
 * Returns the mutated payload when something was stripped, `undefined`
 * otherwise — matching Pi's contract that `undefined` means "no change".
 * Wrapped in try/catch so an unexpected throw (e.g. a future Pi version
 * sending a frozen payload) leaves a stderr breadcrumb instead of being
 * silently swallowed by Pi's handler-error path and surfacing only as a
 * downstream Foundry rejection.
 */
export function stripEagerInputStreaming(payload: unknown): unknown {
	try {
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
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		console.warn(`scramjet: stripEagerInputStreaming failed: ${message}`);
		return undefined;
	}
}

/**
 * Wire the provider bridge into Pi. Reads env (defaults to `process.env`),
 * resolves the bridge config, and — if active — calls `pi.registerProvider`
 * and installs the `before_provider_request` strip hook.
 *
 * Any failure (malformed URL, degenerate hostname, `pi.registerProvider`
 * throwing, hook installation throwing, etc.) is logged to stderr and the
 * function returns without re-throwing. The bridge is the first registration
 * in `index.ts`, so a throw here would tear down task-complete, auto-continue,
 * the diagram tool, and the `/scramjet` command via Pi's factory-level
 * try/catch in `loadExtension`. The bridge failing should not disable the
 * rest of the extension.
 */
export function registerProviderBridge(pi: ExtensionAPI, env: NodeJS.ProcessEnv = process.env): void {
	let config: ProviderBridgeConfig | null;
	try {
		config = resolveProviderBridgeConfig(env);
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		console.warn(`scramjet: provider bridge disabled: ${message}`);
		return;
	}

	if (config === null) return;

	// External-boundary calls below (URL re-parse, pi.registerProvider, pi.on)
	// share the same blast-radius concern as the resolver throw: any escape
	// tears down the rest of the extension via Pi's loadExtension factory
	// catch. The `providerRegistered` flag distinguishes "registerProvider
	// failed" (bridge fully disabled) from "hook registration failed after
	// registerProvider succeeded" (bridge half-installed — provider rewired
	// but the strip hook is absent, so every Foundry request would 400). The
	// latter case needs a distinct log so operators can grep for it.
	let providerRegistered = false;
	try {
		const hostname = new URL(config.baseUrl).hostname;
		const auth = config.apiKey !== undefined ? "present" : "absent";
		console.warn(`scramjet: bridging anthropic provider to ${hostname} (apiKey ${auth})`);

		pi.registerProvider("anthropic", config);
		providerRegistered = true;
		pi.on("before_provider_request", async (event) => stripEagerInputStreaming(event.payload));
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		if (providerRegistered) {
			console.warn(`scramjet: provider bridge partially installed (hook registration failed): ${message}`);
		} else {
			console.warn(`scramjet: provider bridge disabled: ${message}`);
		}
	}
}
