import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { describe, expect, it } from "vitest";
import {
	appendSidebarEntry,
	COMMAND_START_TYPE,
	COMMAND_STATUS_TYPE,
	type CommandStatusData,
	ENABLED_TOGGLE_TYPE,
	parseSlashCommand,
	recordCommandInvocation,
	recordCommandStatus,
	registerHistory,
	replayHistory,
	SIDEBAR_MAX,
	USER_INPUT_PARKED_TYPE,
} from "../history.ts";
import { getActiveCommand } from "../phase-machine.ts";
import type { CommandDef, CommandRegistry, CommandStatusRestingStatus, ScramjetState, SidebarEntry } from "../types.ts";
import { freshState } from "./helpers.ts";

type Handler = (event: unknown, ctx: unknown) => unknown;

function recordingPi() {
	const handlers = new Map<string, Handler[]>();
	const appended: { customType: string; data: unknown }[] = [];
	const pi: any = {
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		appendEntry(customType: string, data: unknown) {
			appended.push({ customType, data });
		},
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, handlers, appended, emit };
}

function ctxWithEntries(entries: SessionEntry[]): any {
	return { sessionManager: { getBranch: () => entries } };
}

function customEntry(customType: string, data: unknown): SessionEntry {
	return { type: "custom", id: `e-${Math.random()}`, parentId: null, timestamp: "0", customType, data } as any;
}

function def(name: string): CommandDef {
	return { name, filePath: `/fake/${name}.md`, body: "" };
}

function registryOf(names: string[]): CommandRegistry {
	return new Map(names.map((n) => [n, def(n)] as const));
}

function cmdStart(command: string, depth = 0, ts = 0): SessionEntry {
	const data: SidebarEntry = { command, origin: "user", depth, timestamp: ts };
	return customEntry(COMMAND_START_TYPE, data);
}

function cmdStatus(commandName: string, status: CommandStatusRestingStatus): SessionEntry {
	const data: CommandStatusData = { commandName, status };
	return customEntry(COMMAND_STATUS_TYPE, data);
}

function userInputParked(commandName: string): SessionEntry {
	return customEntry(USER_INPUT_PARKED_TYPE, { commandName });
}

describe("parseSlashCommand", () => {
	const registry = registryOf(["mach10:push", "mach12:issue-create"]);

	it("returns the qualified name when text starts with a registered slash command", () => {
		expect(parseSlashCommand("/mach10:push", registry)).toBe("mach10:push");
	});

	it("strips args after the first whitespace token", () => {
		expect(parseSlashCommand("/mach12:issue-create 23 4 extra", registry)).toBe("mach12:issue-create");
		expect(parseSlashCommand("/mach10:push\tship it", registry)).toBe("mach10:push");
	});

	it("returns null for unregistered slash commands", () => {
		expect(parseSlashCommand("/unknown-thing", registry)).toBeNull();
	});

	it("returns null for non-slash input", () => {
		expect(parseSlashCommand("hello world", registry)).toBeNull();
		expect(parseSlashCommand(" /mach10:push", registry)).toBeNull();
	});

	it("returns null for a bare slash or slash + whitespace", () => {
		expect(parseSlashCommand("/", registry)).toBeNull();
		expect(parseSlashCommand("/ ", registry)).toBeNull();
	});

	it("returns null when registry is empty", () => {
		expect(parseSlashCommand("/mach10:push", new Map())).toBeNull();
	});
});

describe("appendSidebarEntry", () => {
	function entry(command: string): SidebarEntry {
		return { command, origin: "user", depth: 0, timestamp: 0 };
	}

	it("appends to the end and preserves prior order", () => {
		const log = [entry("a"), entry("b")];
		const out = appendSidebarEntry(log, entry("c"));
		expect(out.map((e) => e.command)).toEqual(["a", "b", "c"]);
	});

	it("returns a new array (no mutation of the input)", () => {
		const log = [entry("a")];
		const out = appendSidebarEntry(log, entry("b"));
		expect(out).not.toBe(log);
		expect(log).toHaveLength(1);
	});

	it(`trims to the last ${SIDEBAR_MAX} entries when the log overflows`, () => {
		const log: SidebarEntry[] = [];
		for (let i = 0; i < SIDEBAR_MAX; i++) log.push(entry(`cmd-${i}`));
		const out = appendSidebarEntry(log, entry("newest"));
		expect(out).toHaveLength(SIDEBAR_MAX);
		expect(out[0].command).toBe("cmd-1");
		expect(out[SIDEBAR_MAX - 1].command).toBe("newest");
	});
});

describe("replayHistory", () => {
	it("returns an empty log and null markers when there are no entries", () => {
		const result = replayHistory([]);
		expect(result.sidebarLog).toEqual([]);
		expect(result.enabled).toBeNull();
		expect(getActiveCommand(result.lifecycle)).toBeNull();
		// issue 88: with no journaled status, the resting phase reconstructs to idle.
		expect(result.lifecycle.phase).toBe("idle");
		expect(result.lifecycle).toEqual({ phase: "idle" });
	});

	it("ignores non-custom and unrelated custom entries", () => {
		const entries: SessionEntry[] = [
			{ type: "message", id: "1", parentId: null, timestamp: "0", message: {} } as any,
			customEntry("some-other-extension", { foo: 1 }),
		];
		const result = replayHistory(entries);
		expect(result.sidebarLog).toEqual([]);
		expect(result.enabled).toBeNull();
	});

	it("rebuilds the sidebar log in insertion order", () => {
		const entries = [cmdStart("a"), cmdStart("b"), cmdStart("c")];
		const result = replayHistory(entries);
		expect(result.sidebarLog.map((e) => e.command)).toEqual(["a", "b", "c"]);
		expect(getActiveCommand(result.lifecycle)).toBe("c");
		expect(result.lifecycle).toEqual({ phase: "dormant", command: "c" });
	});

	it("only updates activeTopLevelCommand from depth-0 entries (nested delegates don't overwrite the top level)", () => {
		// Forward-compat with Stage 5: delegate-sourced entries will carry depth > 0
		// and must not be promoted to activeTopLevelCommand on replay.
		const entries = [cmdStart("top", 0), cmdStart("nested", 1), cmdStart("deeper", 2)];
		const result = replayHistory(entries);
		expect(getActiveCommand(result.lifecycle)).toBe("top");
		expect(result.sidebarLog.map((e) => e.command)).toEqual(["top", "nested", "deeper"]);
	});

	it("returns the latest enabled toggle value", () => {
		const entries: SessionEntry[] = [
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: true }),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: false }),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: true }),
		];
		expect(replayHistory(entries).enabled).toBe(true);
	});

	it("returns null enabled when no toggle entries are present (preserves prior value at the caller)", () => {
		const entries: SessionEntry[] = [cmdStart("a")];
		expect(replayHistory(entries).enabled).toBeNull();
	});

	it("ignores command-start entries with missing or malformed data", () => {
		const entries: SessionEntry[] = [customEntry(COMMAND_START_TYPE, undefined), cmdStart("ok")];
		const result = replayHistory(entries);
		expect(result.sidebarLog.map((e) => e.command)).toEqual(["ok"]);
	});

	it("ignores command-start entries with non-string or empty command field (F10)", () => {
		// A corrupt journal entry where data.command is undefined/null/empty
		// would otherwise set activeTopLevelCommand to a bogus value and break
		// subsequent policy lookups. The TS cast at the read site otherwise
		// hides this from the compiler.
		const entries: SessionEntry[] = [
			cmdStart("first"),
			customEntry(COMMAND_START_TYPE, { origin: "user", depth: 0, timestamp: 0 }),
			customEntry(COMMAND_START_TYPE, { command: "", origin: "user", depth: 0, timestamp: 0 }),
			customEntry(COMMAND_START_TYPE, { command: 42, origin: "user", depth: 0, timestamp: 0 }),
		];
		const result = replayHistory(entries);
		expect(result.sidebarLog.map((e) => e.command)).toEqual(["first"]);
		expect(getActiveCommand(result.lifecycle)).toBe("first");
	});

	it("ignores enabled-toggle entries with missing or malformed data (F36)", () => {
		// Symmetric to the F10 command-start malformed-data filter: a corrupt
		// toggle entry (undefined data, missing `enabled`, non-boolean) must
		// not overwrite state.enabled. We assert by interleaving a valid toggle
		// after the malformed ones — if the filter let any of them through,
		// `enabled` would end up undefined-cast-to-null or the wrong boolean.
		const entries: SessionEntry[] = [
			customEntry(ENABLED_TOGGLE_TYPE, undefined),
			customEntry(ENABLED_TOGGLE_TYPE, {}),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: "yes" }),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: 1 }),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: true }),
		];
		const result = replayHistory(entries);
		expect(result.enabled).toBe(true);
	});

	it("returns null enabled when every toggle entry is malformed (caller preserves prior value)", () => {
		const entries: SessionEntry[] = [
			customEntry(ENABLED_TOGGLE_TYPE, undefined),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: null }),
		];
		expect(replayHistory(entries).enabled).toBeNull();
	});

	it("trims a replayed log of more than SIDEBAR_MAX entries", () => {
		const entries: SessionEntry[] = [];
		for (let i = 0; i < SIDEBAR_MAX + 5; i++) entries.push(cmdStart(`c-${i}`));
		const result = replayHistory(entries);
		expect(result.sidebarLog).toHaveLength(SIDEBAR_MAX);
		expect(result.sidebarLog[0].command).toBe("c-5");
	});
});

// issue 88: replayHistory reconstructs the resumable "waiting" phase from
// journaled COMMAND_STATUS_TYPE entries so a paused interactive command survives
// rewind/resume, while a command that completed (even without chaining) never
// resurrects. Last-status-wins, scoped to the active top-level command.
describe("replayHistory — command-status phase reconstruction (issue 88)", () => {
	it("reconstructs waiting when the active command has a user-input-parked entry", () => {
		const result = replayHistory([cmdStart("a"), userInputParked("a")]);
		expect(result.lifecycle.phase).toBe("waiting");
		expect(getActiveCommand(result.lifecycle)).toBe("a");
		expect(result.lifecycle).toEqual({ phase: "waiting", command: "a" });
	});

	it("reconstructs idle when a waiting command later completed without chaining (no resurrection)", () => {
		// The duplicate-work safety: a command that waited, was answered, then
		// completed without offering a next step writes no subsequent command-start,
		// so journaling the resolving status is what makes the rewind land on idle.
		// issue 128: completed clears activeTopLevelCommand so a later reply
		// doesn't re-arm the phase for a finished command.
		const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStatus("a", "completed")]);
		expect(result.lifecycle.phase).toBe("idle");
		expect(getActiveCommand(result.lifecycle)).toBeNull();
		expect(result.lifecycle).toEqual({ phase: "idle" });
	});

	it.each(["blocked", "incomplete"] as const)(
		"clears the active command when a waiting command later reported %s",
		(status) => {
			const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStatus("a", status)]);
			expect(result.lifecycle.phase).toBe("idle");
			expect(getActiveCommand(result.lifecycle)).toBeNull();
		},
	);

	it("resets to dormant when a new depth-0 command starts after a waiting report", () => {
		// start(B) supersedes A: B has reported nothing yet, so its resting phase is
		// dormant (command associated but not actively running).
		const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStart("b")]);
		expect(result.lifecycle.phase).toBe("dormant");
		expect(getActiveCommand(result.lifecycle)).toBe("b");
		expect(result.lifecycle).toEqual({ phase: "dormant", command: "b" });
	});

	it("ignores a user-input-parked entry whose commandName does not match the active command", () => {
		// A stale entry from a since-superseded command must not move B's phase.
		const result = replayHistory([cmdStart("b"), userInputParked("a")]);
		expect(result.lifecycle.phase).toBe("dormant");
		expect(getActiveCommand(result.lifecycle)).toBe("b");
	});

	it("skips a malformed status entry (missing commandName or out-of-union status)", () => {
		const result = replayHistory([
			cmdStart("a"),
			customEntry(COMMAND_STATUS_TYPE, undefined),
			customEntry(COMMAND_STATUS_TYPE, { status: "completed" }),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "", status: "completed" }),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "a", status: "bogus" }),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "a", status: "continuing" }),
		]);
		// None of the malformed entries reconstruct a waiting phase — remains dormant.
		expect(result.lifecycle.phase).toBe("dormant");
		expect(getActiveCommand(result.lifecycle)).toBe("a");
	});

	it("reconstructs waiting only from the LAST entry when several are journaled", () => {
		const result = replayHistory([cmdStart("a"), cmdStatus("a", "incomplete"), userInputParked("a")]);
		expect(result.lifecycle.phase).toBe("waiting");
	});
});

describe("recordCommandStatus", () => {
	it("appends a COMMAND_STATUS_TYPE journal entry and mutates no state", () => {
		const { pi, appended } = recordingPi();
		recordCommandStatus(pi, "mach12:pr-create", "completed");
		expect(appended).toHaveLength(1);
		expect(appended[0].customType).toBe(COMMAND_STATUS_TYPE);
		expect(appended[0].data as CommandStatusData).toEqual({
			commandName: "mach12:pr-create",
			status: "completed",
		});
	});
});

describe("recordCommandInvocation", () => {
	it("records depth-0 starts as active top-level commands", () => {
		const state = freshState();
		const { pi, appended } = recordingPi();

		recordCommandInvocation(pi, state, "top", "user", 0);

		expect(getActiveCommand(state.lifecycle)).toBe("top");
		expect(state.sidebarLog[0]).toMatchObject({ command: "top", origin: "user", depth: 0 });
		expect((appended[0].data as SidebarEntry).depth).toBe(0);
	});

	it("records delegated depth entries without replacing the lifecycle command", () => {
		const state = freshState({ lifecycle: { phase: "dormant", command: "top" } });
		const { pi, appended } = recordingPi();

		recordCommandInvocation(pi, state, "delegate", "agent", 1);

		expect(getActiveCommand(state.lifecycle)).toBe("top");
		expect(state.sidebarLog[0]).toMatchObject({ command: "delegate", origin: "agent", depth: 1 });
		expect(appended[0].customType).toBe(COMMAND_START_TYPE);
		expect(appended[0].data as SidebarEntry).toMatchObject({ command: "delegate", origin: "agent", depth: 1 });
	});

	it("starts a depth-0 command in the running phase and clears any prior status (issue 84)", () => {
		const state = freshState({
			lifecycle: {
				phase: "reported",
				command: "old",
				status: { status: "completed", summary: "old" },
				continueCount: 0,
			},
		});
		const { pi } = recordingPi();

		recordCommandInvocation(pi, state, "top", "user", 0);

		expect(state.lifecycle.phase).toBe("running");
		expect(state.lifecycle).toEqual({ phase: "running", command: "top", continueCount: 0 });
	});

	it("does not touch the command phase for delegated depth entries (probe stays probing)", () => {
		// The probe turn is not a command start; a delegate during it must not
		// reset the phase, or the status report would be rejected as out-of-phase.
		const state = freshState({ lifecycle: { phase: "probing", command: "top", continueCount: 0 } });
		const { pi } = recordingPi();

		recordCommandInvocation(pi, state, "delegate", "agent", 1);

		expect(state.lifecycle.phase).toBe("probing");
	});
});

describe("registerHistory — handler registration", () => {
	it("registers handlers for input, session_start, session_tree, and before_agent_start", () => {
		const { pi, handlers } = recordingPi();
		registerHistory(pi, freshState());
		expect(handlers.get("input")).toHaveLength(1);
		expect(handlers.get("session_start")).toHaveLength(1);
		expect(handlers.get("session_tree")).toHaveLength(1);
		expect(handlers.get("before_agent_start")).toHaveLength(1);
	});
});

describe("registerHistory — input event", () => {
	async function fire(event: { text: string; source: "interactive" | "extension" | "rpc" }) {
		const state = freshState({ registry: registryOf(["mach10:push", "mach12:issue-create"]) });
		const { pi, appended, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", event);
		return { state, appended };
	}

	it("records a sidebar entry and appendEntry call when a registered slash command is invoked interactively", async () => {
		const { state, appended } = await fire({ text: "/mach10:push", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toBe("mach10:push");
		expect(state.sidebarLog[0].origin).toBe("user");
		expect(state.sidebarLog[0].depth).toBe(0);
		expect(appended).toHaveLength(1);
		expect(appended[0].customType).toBe(COMMAND_START_TYPE);
		expect((appended[0].data as SidebarEntry).command).toBe("mach10:push");
		expect((appended[0].data as SidebarEntry).origin).toBe("user");
	});

	it("marks extension-source invocations as 'agent' origin", async () => {
		const { state, appended } = await fire({ text: "/mach12:issue-create 23", source: "extension" });
		expect(state.sidebarLog[0].origin).toBe("agent");
		expect((appended[0].data as SidebarEntry).origin).toBe("agent");
	});

	it("ignores non-slash input and unregistered slash commands", async () => {
		const plain = await fire({ text: "just typing", source: "interactive" });
		expect(plain.state.sidebarLog).toHaveLength(0);
		expect(plain.appended).toHaveLength(0);

		const unknown = await fire({ text: "/not-registered foo", source: "interactive" });
		expect(unknown.state.sidebarLog).toHaveLength(0);
		expect(unknown.appended).toHaveLength(0);
	});

	it("clears activeTopLevelCommand when the user types an unregistered slash command (F25)", async () => {
		// If the user types a typo or removed-command slash, the previous
		// workflow's next-step policy must not silently apply to whatever
		// the agent produces next.
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: { phase: "dormant", command: "mach10:push" },
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "/typo-or-removed", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBeNull();
	});

	it("does NOT clear activeTopLevelCommand for known Pi/scramjet built-in slash commands (F4)", async () => {
		// /scramjet on, /clear, /help etc. are registered with Pi, not the
		// command-set registry. The user toggling /scramjet on mid-workflow
		// must not silently break the forced chain.
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: { phase: "dormant", command: "mach10:push" },
		});
		const { pi, emit } = recordingPi();
		// Simulate pi.getCommands() returning known commands.
		(pi as any).getCommands = () => [
			{ name: "scramjet", description: "toggle", source: "extension", sourceInfo: {} },
			{ name: "clear", description: "clear", source: "extension", sourceInfo: {} },
		];
		registerHistory(pi, state);
		await emit("input", { text: "/scramjet on", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
		await emit("input", { text: "/clear", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
	});

	it("falls back to allow-list when pi.getCommands is unavailable (F4)", async () => {
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: { phase: "dormant", command: "mach10:push" },
		});
		const { pi, emit } = recordingPi();
		// No getCommands on the fake pi — tests fallback allow-list path.
		registerHistory(pi, state);
		await emit("input", { text: "/scramjet on", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
		await emit("input", { text: "/clear", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
		// Removed/internal or unknown slashes are not allow-listed.
		await emit("input", { text: "/scramjet-exec-fresh foo", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBeNull();
	});

	it("leaves activeTopLevelCommand alone for non-slash input (continuing a conversation)", async () => {
		// Plain follow-up text must not nuke the active workflow — otherwise
		// any chat after the command would disable next-step auto-continue.
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: { phase: "dormant", command: "mach10:push" },
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "plain follow-up", source: "interactive" });
		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
	});

	it("labels origin 'forced' and clears state.pendingForcedDispatch when the dispatched command matches", async () => {
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			pendingForcedDispatch: "mach10:push",
		});
		const { pi, appended, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "/mach10:push", source: "extension" });

		expect(state.sidebarLog[0].origin).toBe("forced");
		expect((appended[0].data as SidebarEntry).origin).toBe("forced");
		expect(state.pendingForcedDispatch).toBeNull();
	});

	it("does not set activeTopLevelCommand for a known non-Scramjet slash command", async () => {
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: { phase: "dormant", command: "mach10:push" },
		});
		const { pi, emit } = recordingPi();
		(pi as any).getCommands = () => [{ name: "other-extension:cmd" }];
		registerHistory(pi, state);

		await emit("input", { text: "/other-extension:cmd --flag", source: "extension" });

		expect(getActiveCommand(state.lifecycle)).toBe("mach10:push");
		expect(state.sidebarLog).toEqual([]);
	});

	// issue 88: resuming a paused (waiting) command. An interactive, non-slash
	// reply while the active command rests at "waiting" re-arms the probe path by
	// flipping the phase back to "running"; the resulting turn's agent_end fires
	// the existing running→probing probe.
	describe("resume a paused command (issue 88)", () => {
		function waitingState() {
			return freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: { phase: "waiting", command: "mach12:pr-create" },
			});
		}

		it("flips waiting→running on an interactive non-slash reply", async () => {
			const state = waitingState();
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "approve", source: "interactive" });
			expect(state.lifecycle.phase).toBe("running");
			expect(getActiveCommand(state.lifecycle)).toBe("mach12:pr-create");
			expect(state.lifecycle).toEqual({ phase: "running", command: "mach12:pr-create", continueCount: 0 });
			// A resume is not a fresh command start: no sidebar/journal entry.
			expect(state.sidebarLog).toHaveLength(0);
		});

		it("does not resume on an extension-source non-slash reply (only interactive)", async () => {
			// The hidden status probe sends via triggerTurn (bypassing the input
			// pipeline), but extension-dispatched input does flow through it; gating
			// on source === "interactive" keeps non-user input from self-resuming.
			const state = waitingState();
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "approve", source: "extension" });
			expect(state.lifecycle.phase).toBe("waiting");
		});

		it("treats a registered slash command while waiting as a normal command start", async () => {
			const state = waitingState();
			const { pi, appended, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "/mach12:pr-create", source: "interactive" });
			// recordCommandStart fires: phase running, active set, journaled.
			expect(state.lifecycle.phase).toBe("running");
			expect(getActiveCommand(state.lifecycle)).toBe("mach12:pr-create");
			expect(appended).toHaveLength(1);
		});

		it("drops the waiting phase to idle when an unknown slash exits the workflow (F25)", async () => {
			const state = waitingState();
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "/typo-or-removed", source: "interactive" });
			expect(getActiveCommand(state.lifecycle)).toBeNull();
			expect(state.lifecycle.phase).toBe("idle");
			expect(state.lifecycle).toEqual({ phase: "idle" });
		});

		it("re-arms dormant→running on an interactive non-slash reply when lifecycle is dormant (issue 128)", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: { phase: "dormant", command: "mach12:pr-create" },
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "just chatting", source: "interactive" });
			expect(state.lifecycle.phase).toBe("running");
			expect(getActiveCommand(state.lifecycle)).toBe("mach12:pr-create");
			expect(state.lifecycle).toEqual({ phase: "running", command: "mach12:pr-create", continueCount: 0 });
			// A resume is not a fresh command start: no sidebar/journal entry.
			expect(state.sidebarLog).toHaveLength(0);
		});

		it("does not re-arm idle→running when lifecycle is idle", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: { phase: "idle" },
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "just chatting", source: "interactive" });
			expect(state.lifecycle.phase).toBe("idle");
			expect(state.lifecycle).toEqual({ phase: "idle" });
		});

		it("does not re-arm dormant→running on extension-source replies", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: { phase: "dormant", command: "mach12:pr-create" },
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "approve", source: "extension" });
			expect(state.lifecycle.phase).toBe("dormant");
			expect(state.lifecycle).toEqual({ phase: "dormant", command: "mach12:pr-create" });
		});
	});

	it("does not consume the forced flag when the dispatched name differs", async () => {
		const state = freshState({
			registry: registryOf(["mach10:push", "mach10:other"]),
			pendingForcedDispatch: "mach10:push",
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "/mach10:other", source: "extension" });

		// Different command — labeled per source, flag preserved for the
		// still-pending forced dispatch.
		expect(state.sidebarLog[0].origin).toBe("agent");
		expect(state.pendingForcedDispatch).toBe("mach10:push");
	});
});

describe("registerHistory — replay on session events", () => {
	function setup(entries: SessionEntry[], initial: Partial<ScramjetState> = {}) {
		const state = freshState(initial);
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		const ctx = ctxWithEntries(entries);
		return { state, emit, ctx };
	}

	it("rebuilds sidebarLog and activeTopLevelCommand from the branch on session_start", async () => {
		const { state, emit, ctx } = setup([cmdStart("a"), cmdStart("b")]);
		await emit("session_start", {}, ctx);
		expect(state.sidebarLog.map((e) => e.command)).toEqual(["a", "b"]);
		expect(getActiveCommand(state.lifecycle)).toBe("b");
	});

	it("rebuilds state on session_tree the same way as session_start", async () => {
		const { state, emit, ctx } = setup([cmdStart("only")]);
		await emit("session_tree", {}, ctx);
		expect(state.sidebarLog.map((e) => e.command)).toEqual(["only"]);
		expect(getActiveCommand(state.lifecycle)).toBe("only");
	});

	it("applies the latest enabled toggle from the branch", async () => {
		const entries: SessionEntry[] = [
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: true }),
			cmdStart("a"),
			customEntry(ENABLED_TOGGLE_TYPE, { enabled: false }),
		];
		const { state, emit, ctx } = setup(entries);
		state.enabled = true;
		await emit("session_start", {}, ctx);
		expect(state.enabled).toBe(false);
	});

	it("leaves state.enabled unchanged when the replayed branch has no toggle entries", async () => {
		const { state, emit, ctx } = setup([cmdStart("a")], { enabled: true });
		await emit("session_start", {}, ctx);
		expect(state.enabled).toBe(true);
	});

	it("clears pendingForcedDispatch on session rebuild (F18 defense)", async () => {
		// Transient runtime flag; meaningless after navigation/resume. A stale
		// value could mislabel a later user-typed slash as origin: "forced".
		const { state, emit, ctx } = setup([cmdStart("a")], { pendingForcedDispatch: "stale:target" });
		await emit("session_start", {}, ctx);
		expect(state.pendingForcedDispatch).toBeNull();
	});

	it("self-heals the command phase to idle on rebuild (issue 84 resume safety)", async () => {
		// A session resumed mid-probe must not replay a "probing" phase with no
		// live probe turn behind it. Reset to idle so a stale status tool call is
		// rejected by the phase guard instead of mis-dispatching.
		const { state, emit, ctx } = setup([cmdStart("a")], {
			lifecycle: { phase: "probing", command: "stale", continueCount: 0 },
		});
		await emit("session_start", {}, ctx);
		// Replay reconstructs to dormant (command-start "a" present, no terminal status).
		expect(state.lifecycle.phase).toBe("dormant");
	});

	// issue 88 / issue 156: a paused command survives rewind/resume. The
	// journaled user-input-parked entry reconstructs the stable "waiting" phase
	// so a later interactive reply can resume the command.
	it("reconstructs the waiting phase on rebuild when the active command has a user-input-parked entry", async () => {
		const { state, emit, ctx } = setup([cmdStart("a"), userInputParked("a")], {
			lifecycle: { phase: "probing", command: "stale", continueCount: 0 },
		});
		await emit("session_start", {}, ctx);
		expect(state.lifecycle.phase).toBe("waiting");
		expect(getActiveCommand(state.lifecycle)).toBe("a");
	});

	it("reconstructs idle on rebuild when a waiting command later completed (no resurrection)", async () => {
		const { state, emit, ctx } = setup([cmdStart("a"), userInputParked("a"), cmdStatus("a", "completed")], {
			lifecycle: { phase: "running", command: "stale", continueCount: 0 },
		});
		await emit("session_start", {}, ctx);
		expect(state.lifecycle.phase).toBe("idle");
	});
});

describe("registerHistory — before_agent_start turn boundary", () => {
	it("clears pendingForcedDispatch at the next agent turn (F18 turn boundary)", async () => {
		// If the forced target wasn't in the registry, the input handler can't
		// consume the flag (parseSlashCommand returns null). The before_agent_start
		// of the resulting turn is the latest moment we can guarantee the flag
		// is stale; clearing here keeps the flag's lifetime bounded to one turn.
		const state = freshState({ pendingForcedDispatch: "orphan:target" });
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("before_agent_start", {}, {});
		expect(state.pendingForcedDispatch).toBeNull();
	});
});
