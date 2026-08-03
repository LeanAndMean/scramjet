import type { ThinkingLevel } from "@leanandmean/agent";
import type { Api, Model } from "@leanandmean/ai";
import "@leanandmean/coding-agent";
import { KeybindingsManager, TUI_KEYBINDINGS, visibleWidth } from "@leanandmean/tui";
import { describe, expect, it, vi } from "vitest";
import { createSelectorEffortControl } from "../src/selector-effort.js";

const EFFORT = "\x1b[Z";
const ALT_E = "\x1be";
const ENTER = "\r";

function model(reasoning: boolean, thinkingLevelMap?: Model<Api>["thinkingLevelMap"]): Model<Api> {
	return {
		provider: "test",
		id: "model",
		name: "Model",
		api: "anthropic-messages",
		baseUrl: "",
		reasoning,
		thinkingLevelMap,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 200000,
		maxTokens: 8192,
	};
}

function keybindings(effort: string | string[] | undefined = "shift+tab", confirm = "enter") {
	return new KeybindingsManager(
		{
			...TUI_KEYBINDINGS,
			"app.thinking.cycle": { defaultKeys: "shift+tab" },
		},
		{
			"app.thinking.cycle": effort,
			"tui.select.confirm": confirm,
		},
	);
}

function thinking(initial: ThinkingLevel = "high", clamp?: (level: ThinkingLevel) => ThinkingLevel) {
	let level = initial;
	const setThinkingLevel = vi.fn((next: ThinkingLevel) => {
		level = clamp?.(next) ?? next;
	});
	return {
		api: { getThinkingLevel: () => level, setThinkingLevel },
		get level() {
			return level;
		},
		setThinkingLevel,
	};
}

const protectedActions = ["tui.select.up", "tui.select.down", "tui.select.confirm", "tui.select.cancel"] as const;

describe("createSelectorEffortControl", () => {
	it("matches configured bindings, cycles supported levels, and wraps", () => {
		const state = thinking("max");
		const control = createSelectorEffortControl({
			model: model(true, { xhigh: "xhigh", max: "max" }),
			thinking: state.api,
			keybindings: keybindings(["alt+e", "shift+tab"]),
			protectedActions,
		});

		expect(control.handleInput(ALT_E)).toBe(true);
		expect(state.level).toBe("off");
		expect(control.handleInput(EFFORT)).toBe(true);
		expect(state.level).toBe("minimal");
		expect(control.render(80)).toContain("alt+e/shift+tab");
	});

	it("requests the first supported level when the current level is absent", () => {
		const state = thinking("max");
		const control = createSelectorEffortControl({
			model: model(true, { xhigh: null, max: null }),
			thinking: state.api,
			keybindings: keybindings(),
			protectedActions,
		});

		expect(control.handleInput(EFFORT)).toBe(true);
		expect(state.setThinkingLevel).toHaveBeenCalledWith("off");
	});

	it("rereads the effective value after the setter clamps it", () => {
		const state = thinking("low", () => "off");
		const control = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings(),
			protectedActions,
		});

		control.handleInput(EFFORT);
		expect(control.render(80)).toContain("effort: off");
	});

	it.each([
		["undefined model", undefined],
		["off-only model", model(false)],
	] as const)("consumes configured input without setting effort for %s", (_name, committedModel) => {
		const state = thinking();
		const control = createSelectorEffortControl({
			model: committedModel,
			thinking: state.api,
			keybindings: keybindings(),
			protectedActions,
		});

		expect(control.handleInput(EFFORT)).toBe(true);
		expect(state.setThinkingLevel).not.toHaveBeenCalled();
	});

	it("filters conflicting bindings from the hint and refuses protected input", () => {
		const state = thinking();
		const partial = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings(["enter", "shift+tab"]),
			protectedActions,
		});

		expect(partial.handleInput(ENTER)).toBe(false);
		expect(state.setThinkingLevel).not.toHaveBeenCalled();
		expect(partial.render(80)).toContain("shift+tab");
		expect(partial.render(80)).not.toContain("enter");

		const all = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings("enter"),
			protectedActions,
		});
		expect(all.render(80)).toBe("effort: high");
	});

	it.each([
		["return", "enter", ENTER],
		["esc", "escape", "\x1b"],
		["ctrl+shift+p", "shift+ctrl+p", undefined],
		["ctrl+[", "escape", "\x1b"],
		["ctrl+m", "enter", ENTER],
		["ctrl+j", "enter", "\n"],
	] as const)("filters matcher-equivalent %s/%s conflicts from the hint", (effort, protectedKey, input) => {
		const state = thinking();
		const control = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings(effort, protectedKey),
			protectedActions,
		});

		expect(control.render(80)).toBe("effort: high");
		if (input !== undefined) {
			expect(control.handleInput(input)).toBe(false);
			expect(state.setThinkingLevel).not.toHaveBeenCalled();
		}
	});

	it("omits the shortcut when unbound", () => {
		const state = thinking();
		const control = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings([]),
			protectedActions,
		});

		expect(control.render(80)).toBe("effort: high");
	});

	it("keeps styled status within the supplied width", () => {
		const state = thinking();
		const control = createSelectorEffortControl({
			model: model(true),
			thinking: state.api,
			keybindings: keybindings(["alt+e", "shift+tab"]),
			protectedActions,
		});
		const line = control.render(12, (text) => `\x1b[2m${text}\x1b[0m`);

		expect(visibleWidth(line)).toBeLessThanOrEqual(12);
	});
});
