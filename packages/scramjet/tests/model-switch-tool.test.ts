import { describe, expect, it, vi } from "vitest";
import { registerModelSwitchTool } from "../src/model-switch-tool.js";
import type { ScramjetState } from "../src/types.js";
import { freshState, recordingPi } from "./helpers.js";

type SwitchParams = { provider: string; model: string };

interface FakeModel {
	provider: string;
	id: string;
	name: string;
}

// The registry surface the tool relies on: find(provider, id) and getAvailable().
function fakeModelRegistry(models: FakeModel[], available: FakeModel[] = models) {
	return {
		find: (provider: string, id: string) => models.find((m) => m.provider === provider && m.id === id),
		getAvailable: () => available,
	};
}

const CLAUDE: FakeModel = { provider: "anthropic", id: "claude-opus-4-8", name: "Claude Opus 4.8" };
const GPT: FakeModel = { provider: "openai", id: "gpt-5.5", name: "GPT 5.5" };

function toolFor(
	options: {
		state?: ScramjetState;
		setModel?: (model: unknown) => Promise<boolean>;
		registry?: ReturnType<typeof fakeModelRegistry>;
	} = {},
) {
	const state = options.state ?? freshState();
	const { pi, tools } = recordingPi();
	pi.setModel = options.setModel ?? vi.fn(async () => true);
	registerModelSwitchTool(pi, state);
	const tool = tools.find((t: any) => t.name === "switch_scramjet_model");
	if (!tool) throw new Error("switch_scramjet_model tool not registered");
	const registry = options.registry ?? fakeModelRegistry([CLAUDE, GPT]);
	const execute = (params: SwitchParams) =>
		tool.execute("call-id", params, undefined, undefined, { modelRegistry: registry } as any) as Promise<any>;
	return { state, pi, tool, execute };
}

function resultText(result: any): string {
	return result.content.map((c: any) => c.text).join("\n");
}

describe("switch_scramjet_model registration", () => {
	it("registers a harness tool with a prompt snippet", () => {
		const { tool } = toolFor();
		expect(tool.name).toBe("switch_scramjet_model");
		expect(typeof tool.promptSnippet).toBe("string");
		expect(tool.promptSnippet.length).toBeGreaterThan(0);
	});
});

describe("switch_scramjet_model valid switch", () => {
	it("calls pi.setModel with the registry-resolved model object", async () => {
		const setModel = vi.fn(async () => true);
		const { execute } = toolFor({ setModel });

		const result = await execute({ provider: "anthropic", model: "claude-opus-4-8" });

		expect(setModel).toHaveBeenCalledTimes(1);
		expect(setModel).toHaveBeenCalledWith(CLAUDE);
		expect(result.details).toMatchObject({ switched: true, provider: "anthropic", model: "claude-opus-4-8" });
		expect(resultText(result)).toContain("Claude Opus 4.8");
		expect(result.terminate).toBeUndefined();
	});

	it("sets the suppression flag before the setModel call", async () => {
		const state = freshState();
		let flagWhenCalled: boolean | undefined;
		const setModel = vi.fn(async () => {
			flagWhenCalled = state.suppressNextModelNotify;
			return true;
		});
		const { execute } = toolFor({ state, setModel });

		await execute({ provider: "anthropic", model: "claude-opus-4-8" });

		// Flag must be observable as true at the moment setModel (and its
		// synchronous model_select emission) runs.
		expect(flagWhenCalled).toBe(true);
	});
});

describe("switch_scramjet_model unknown model", () => {
	it("does not call setModel and leaves state untouched", async () => {
		const state = freshState();
		const setModel = vi.fn(async () => true);
		const { execute } = toolFor({ state, setModel });

		const result = await execute({ provider: "anthropic", model: "no-such-model" });

		expect(setModel).not.toHaveBeenCalled();
		expect(result.details).toMatchObject({ error: "unknown-model" });
		expect(state.suppressNextModelNotify).toBe(false);
		expect(result.terminate).toBeUndefined();
	});

	it("lists the available catalog in the error text", async () => {
		const registry = fakeModelRegistry([CLAUDE, GPT], [CLAUDE]);
		const { execute } = toolFor({ registry });

		const result = await execute({ provider: "anthropic", model: "no-such-model" });

		const text = resultText(result);
		expect(text).toContain("anthropic/claude-opus-4-8");
		// Catalog reflects getAvailable(), not the full registry.
		expect(text).not.toContain("openai/gpt-5.5");
	});
});

describe("switch_scramjet_model unauthorized model", () => {
	it("reports an error and clears the suppression flag when setModel returns false", async () => {
		const state = freshState();
		const setModel = vi.fn(async () => false);
		const { execute } = toolFor({ state, setModel });

		const result = await execute({ provider: "openai", model: "gpt-5.5" });

		expect(setModel).toHaveBeenCalledTimes(1);
		expect(result.details).toMatchObject({ error: "no-auth", provider: "openai", model: "gpt-5.5" });
		expect(resultText(result)).toContain("no API key");
		// Flag was set before the call and must be cleared once the switch failed.
		expect(state.suppressNextModelNotify).toBe(false);
	});
});

describe("switch_scramjet_model same-model guard", () => {
	it("returns a no-op when the target matches the current model, never setting the suppression flag", async () => {
		const state = freshState({
			currentModel: { name: CLAUDE.name, id: CLAUDE.id, provider: CLAUDE.provider, fromTurnIndex: 0 },
			modelHistory: [{ name: CLAUDE.name, id: CLAUDE.id, provider: CLAUDE.provider, fromTurnIndex: 0 }],
		});
		const setModel = vi.fn(async () => true);
		const { execute } = toolFor({ state, setModel });

		const result = await execute({ provider: "anthropic", model: "claude-opus-4-8" });

		expect(setModel).not.toHaveBeenCalled();
		expect(result.details).toMatchObject({ switched: false, reason: "already-active" });
		expect(resultText(result)).toContain("Already on");
		// The suppression flag must never be set — a stranded flag would swallow
		// the next user-initiated model-change notice.
		expect(state.suppressNextModelNotify).toBe(false);
	});
});

describe("switch_scramjet_model setModel throws", () => {
	it("clears the suppression flag and returns a soft error", async () => {
		const state = freshState();
		const setModel = vi.fn(async () => {
			throw new Error("persist failed");
		});
		const { execute } = toolFor({ state, setModel });

		const result = await execute({ provider: "anthropic", model: "claude-opus-4-8" });

		expect(result.details).toMatchObject({ error: "switch-failed" });
		expect(resultText(result)).toContain("persist failed");
		expect(state.suppressNextModelNotify).toBe(false);
	});
});
