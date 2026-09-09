import type { Api } from "./types.js";

export type ProviderRequestToolInventoryMalformedReason =
	| "payload-not-object"
	| "payload-unreadable"
	| "tool-container-not-object"
	| "tool-list-not-array"
	| "tool-entry-not-object"
	| "tool-name-missing"
	| "tool-name-not-string"
	| "tool-name-empty";

export type ProviderRequestToolInventory =
	| { readonly status: "observed"; readonly toolNames: readonly string[] }
	| { readonly status: "malformed"; readonly reason: ProviderRequestToolInventoryMalformedReason }
	| { readonly status: "unsupported" };

type MalformedInventory = Extract<ProviderRequestToolInventory, { status: "malformed" }>;
type NameResult = string | MalformedInventory;
type RecordValue = Record<string, unknown>;

function isRecord(value: unknown): value is RecordValue {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function malformed(reason: ProviderRequestToolInventoryMalformedReason): MalformedInventory {
	return Object.freeze({ status: "malformed", reason });
}

function observed(names: string[]): ProviderRequestToolInventory {
	return Object.freeze({
		status: "observed",
		toolNames: Object.freeze([...new Set(names)].sort()),
	});
}

function readName(value: RecordValue): NameResult {
	if (!("name" in value) || value.name === undefined) return malformed("tool-name-missing");
	if (typeof value.name !== "string") return malformed("tool-name-not-string");
	if (value.name.trim().length === 0) return malformed("tool-name-empty");
	return value.name;
}

function inspectToolList(
	value: unknown,
	readEntryName: (entry: RecordValue) => NameResult,
): ProviderRequestToolInventory {
	if (value === undefined) return observed([]);
	if (!Array.isArray(value)) return malformed("tool-list-not-array");

	const names: string[] = [];
	for (const entry of value) {
		if (!isRecord(entry)) return malformed("tool-entry-not-object");
		const name = readEntryName(entry);
		if (typeof name !== "string") return name;
		names.push(name);
	}
	return observed(names);
}

function inspectDirectTools(payload: RecordValue): ProviderRequestToolInventory {
	return inspectToolList(payload.tools, readName);
}

function inspectFunctionTools(payload: RecordValue): ProviderRequestToolInventory {
	return inspectToolList(payload.tools, (entry) => {
		if (!isRecord(entry.function)) return malformed("tool-container-not-object");
		return readName(entry.function);
	});
}

function inspectGoogleTools(payload: RecordValue): ProviderRequestToolInventory {
	if (payload.config === undefined) return observed([]);
	if (!isRecord(payload.config)) return malformed("tool-container-not-object");
	if (payload.config.tools === undefined) return observed([]);
	if (!Array.isArray(payload.config.tools)) return malformed("tool-list-not-array");

	const names: string[] = [];
	for (const group of payload.config.tools) {
		if (!isRecord(group)) return malformed("tool-entry-not-object");
		if (!Array.isArray(group.functionDeclarations)) return malformed("tool-list-not-array");
		for (const declaration of group.functionDeclarations) {
			if (!isRecord(declaration)) return malformed("tool-entry-not-object");
			const name = readName(declaration);
			if (typeof name !== "string") return name;
			names.push(name);
		}
	}
	return observed(names);
}

function inspectBedrockTools(payload: RecordValue): ProviderRequestToolInventory {
	if (payload.toolConfig === undefined) return observed([]);
	if (!isRecord(payload.toolConfig)) return malformed("tool-container-not-object");
	if (payload.toolConfig.tools === undefined) return malformed("tool-list-not-array");
	return inspectToolList(payload.toolConfig.tools, (entry) => {
		if (!isRecord(entry.toolSpec)) return malformed("tool-container-not-object");
		return readName(entry.toolSpec);
	});
}

// SCRAMJET-DIVERGENCE: expose privacy-safe names-only evidence from supported built-in request shapes (#524).
export function inspectProviderRequestToolInventory(api: Api, payload: unknown): ProviderRequestToolInventory {
	try {
		switch (api) {
			case "openai-responses":
			case "azure-openai-responses":
			case "openai-codex-responses":
			case "anthropic-messages":
				if (!isRecord(payload)) return malformed("payload-not-object");
				return inspectDirectTools(payload);
			case "openai-completions":
			case "mistral-conversations":
				if (!isRecord(payload)) return malformed("payload-not-object");
				return inspectFunctionTools(payload);
			case "google-generative-ai":
			case "google-vertex":
				if (!isRecord(payload)) return malformed("payload-not-object");
				return inspectGoogleTools(payload);
			case "bedrock-converse-stream":
				if (!isRecord(payload)) return malformed("payload-not-object");
				return inspectBedrockTools(payload);
			default:
				return Object.freeze({ status: "unsupported" });
		}
	} catch {
		return malformed("payload-unreadable");
	}
}
