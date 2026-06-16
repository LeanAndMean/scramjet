/**
 * /scramjet on|off|status writer-side tests.
 *
 * The toggle is the user's only knob for auto-continuation; before this
 * file existed only the *replay* side (history.ts → state.enabled) was
 * covered, so a regression that broke the appendEntry call or the
 * arg-parsing surface would have shipped silently. (F31)
 */

import { describe, expect, it } from "vitest";
import { ENABLED_TOGGLE_TYPE, type EnabledToggleData } from "../history.ts";
import { registerScramjetCommand } from "../scramjet-command.ts";
import { freshState } from "./helpers.ts";

interface RegisteredCommand {
	name: string;
	spec: {
		description?: string;
		getArgumentCompletions?: (prefix: string) => Array<{ value: string; label: string }> | null;
		handler: (args: string, ctx: unknown) => Promise<void> | void;
	};
}

function recordingPi() {
	const commands: RegisteredCommand[] = [];
	const appended: { type: string; data: unknown }[] = [];
	const pi: any = {
		registerCommand(name: string, spec: RegisteredCommand["spec"]) {
			commands.push({ name, spec });
		},
		appendEntry(type: string, data: unknown) {
			appended.push({ type, data });
		},
	};
	return { pi, commands, appended };
}

function fakeCtx() {
	const notifications: { message: string; type?: string }[] = [];
	const ctx: any = {
		ui: {
			notify(message: string, type?: string) {
				notifications.push({ message, type });
			},
		},
	};
	return { ctx, notifications };
}

function spec(commands: RegisteredCommand[]) {
	const entry = commands.find((c) => c.name === "scramjet");
	if (!entry) throw new Error("scramjet command not registered");
	return entry.spec;
}

describe("registerScramjetCommand — registration surface", () => {
	it("registers exactly one command, named 'scramjet', with a description", () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		expect(commands).toHaveLength(1);
		expect(commands[0].name).toBe("scramjet");
		expect(commands[0].spec.description).toBeTruthy();
	});

	it("argument completions: 'o' offers both on/off; 'on' offers only on; 'x' returns null", () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const fn = spec(commands).getArgumentCompletions;
		expect(fn).toBeDefined();
		const completionsFor = (prefix: string) => fn?.(prefix);

		const oResults = completionsFor("o");
		expect(oResults?.map((c) => c.value)).toEqual(["on", "off"]);

		const onResults = completionsFor("on");
		expect(onResults?.map((c) => c.value)).toEqual(["on"]);

		expect(completionsFor("x")).toBeNull();
	});
});

describe("registerScramjetCommand — handler", () => {
	it("'/scramjet on' sets state.enabled=true, appends a toggle entry, and notifies", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: false });
		registerScramjetCommand(pi, state);
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("on", ctx);

		expect(state.enabled).toBe(true);
		expect(appended).toEqual([{ type: ENABLED_TOGGLE_TYPE, data: { enabled: true } satisfies EnabledToggleData }]);
		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("info");
		expect(notifications[0].message.toLowerCase()).toContain("enabled");
	});

	it("'/scramjet off' sets state.enabled=false, appends a toggle entry, and notifies", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: true });
		registerScramjetCommand(pi, state);
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("off", ctx);

		expect(state.enabled).toBe(false);
		expect(appended).toEqual([{ type: ENABLED_TOGGLE_TYPE, data: { enabled: false } satisfies EnabledToggleData }]);
		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("info");
		expect(notifications[0].message.toLowerCase()).toContain("disabled");
	});

	it("arg parsing is case-insensitive and trims surrounding whitespace ('  ON  ')", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: false });
		registerScramjetCommand(pi, state);
		const { ctx } = fakeCtx();
		await spec(commands).handler("  ON  ", ctx);
		expect(state.enabled).toBe(true);
		expect(appended).toHaveLength(1);
	});

	it("'/scramjet' (empty args) reports status without writing a toggle entry", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: true });
		registerScramjetCommand(pi, state);
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("", ctx);

		expect(state.enabled).toBe(true); // unchanged
		expect(appended).toEqual([]); // no journal write for a status query
		expect(notifications).toHaveLength(1);
		expect(notifications[0].message.toLowerCase()).toContain("on");
	});

	it("'/scramjet status' reports off when state is off, no journal write", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: false });
		registerScramjetCommand(pi, state);
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("status", ctx);

		expect(appended).toEqual([]);
		expect(notifications[0].message.toLowerCase()).toContain("off");
	});

	it("unknown args produce a usage warning and do not mutate state or journal", async () => {
		const { pi, commands, appended } = recordingPi();
		const state = freshState({ enabled: false });
		registerScramjetCommand(pi, state);
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("yes please", ctx);

		expect(state.enabled).toBe(false);
		expect(appended).toEqual([]);
		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("warning");
		expect(notifications[0].message).toContain("Usage:");
	});

	it("'/scramjet settings' without TUI notifies error and does not throw", async () => {
		const { pi, commands } = recordingPi();
		const state = freshState();
		registerScramjetCommand(pi, state);
		const notifications: { message: string; type?: string }[] = [];
		const ctx: any = {
			hasUI: false,
			ui: {
				notify(message: string, type?: string) {
					notifications.push({ message, type });
				},
			},
		};

		await spec(commands).handler("settings", ctx);

		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("error");
		expect(notifications[0].message).toContain("TUI");
	});

	it("'settings' appears in argument completions", () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const fn = spec(commands).getArgumentCompletions;
		const results = fn?.("s");
		expect(results?.map((c) => c.value)).toContain("settings");
	});
});
