import type { ExtensionAPI, ProviderRequestToolInventoryEvent } from "@leanandmean/coding-agent";
import { derivePhaseLabel } from "./lifecycle.js";
import type { ScramjetState } from "./types.js";

const AMBIGUOUS_ANTHROPIC_TOOL_NAME = "ambiguous-anthropic-tool-name";

export type ToolVisibilityClassification =
	| {
			classification: "parity" | "mismatch";
			requestContextToolNames: string[];
			serializedToolNames: string[];
			missingNames: string[];
			unexpectedNames: string[];
	  }
	| {
			classification: "uninspectable";
			requestContextToolNames: string[];
			serializedToolNames?: string[];
			inventoryStatus?: "malformed" | "unsupported";
			reason?: string;
	  };

function sortedUnique(names: readonly string[]): string[] {
	return [...new Set(names)].sort();
}

export function classifyToolVisibility(
	event: Pick<ProviderRequestToolInventoryEvent, "model" | "requestContextToolNames" | "inventory">,
): ToolVisibilityClassification {
	const requestContextToolNames = sortedUnique(event.requestContextToolNames);
	if (event.inventory.status === "malformed") {
		return {
			classification: "uninspectable",
			requestContextToolNames,
			inventoryStatus: "malformed",
			reason: event.inventory.reason,
		};
	}
	if (event.inventory.status === "unsupported") {
		return { classification: "uninspectable", requestContextToolNames, inventoryStatus: "unsupported" };
	}

	const serializedToolNames = sortedUnique(event.inventory.toolNames);
	const matchedExpected = new Set<string>();
	const matchedSerialized = new Set<string>();
	for (const serializedName of serializedToolNames) {
		if (!requestContextToolNames.includes(serializedName)) continue;
		matchedExpected.add(serializedName);
		matchedSerialized.add(serializedName);
	}

	const unexpectedNames: string[] = [];
	for (const serializedName of serializedToolNames) {
		if (matchedSerialized.has(serializedName)) continue;
		if (event.model.api !== "anthropic-messages") {
			unexpectedNames.push(serializedName);
			continue;
		}
		const foldedMatches = requestContextToolNames.filter(
			(requestName) =>
				!matchedExpected.has(requestName) && requestName.toLowerCase() === serializedName.toLowerCase(),
		);
		if (foldedMatches.length > 1) {
			return {
				classification: "uninspectable",
				requestContextToolNames,
				serializedToolNames,
				reason: AMBIGUOUS_ANTHROPIC_TOOL_NAME,
			};
		}
		const foldedMatch = foldedMatches[0];
		if (foldedMatch !== undefined && !matchedExpected.has(foldedMatch)) matchedExpected.add(foldedMatch);
		else unexpectedNames.push(serializedName);
	}

	const missingNames = requestContextToolNames.filter((name) => !matchedExpected.has(name));
	return {
		classification: missingNames.length === 0 && unexpectedNames.length === 0 ? "parity" : "mismatch",
		requestContextToolNames,
		serializedToolNames,
		missingNames,
		unexpectedNames,
	};
}

export function registerToolVisibilityDiagnostics(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("provider_request_tool_inventory", (event) => {
		const result = classifyToolVisibility(event);
		const data = {
			provider: event.model.provider,
			model: event.model.id,
			api: event.model.api,
			lifecycleGeneration: state.lifecycleGeneration,
			phase: derivePhaseLabel(state.lifecycle),
			...result,
		};
		if (result.classification === "mismatch") {
			state.logger.warn("tool-visibility", "provider tool inventory mismatch", data);
			return;
		}
		state.logger.debug(
			"tool-visibility",
			result.classification === "parity"
				? "provider tool inventory parity"
				: "provider tool inventory uninspectable",
			data,
		);
	});
}
