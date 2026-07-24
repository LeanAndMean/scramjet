import type { SessionEntry } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import {
	appendSidebarEntry,
	COMMAND_EXIT_TYPE,
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
	STRUCTURED_INPUT_CANCELLATION_TYPE,
	USER_INPUT_PARKED_TYPE,
} from "../src/history.js";
import { activeCommandName } from "../src/lifecycle.js";
import type {
	CommandDef,
	CommandRegistry,
	CommandStatusRestingStatus,
	ScramjetState,
	SidebarEntry,
} from "../src/types.js";
import { derivedPhase, freshState, lifecycleFor } from "./helpers.js";

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

function cmdStatus(commandName: string, status: CommandStatusRestingStatus, summary = "work summary"): SessionEntry {
	const data: CommandStatusData = { commandName, status, summary };
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
		expect(activeCommandName(result.lifecycle)).toBeNull();
		// issue 88: with no journaled status, the resting phase reconstructs to idle.
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(derivedPhase(result.lifecycle)).toBe("idle");
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
		expect(activeCommandName(result.lifecycle)).toBe("c");
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(result.lifecycle.activeCommand).toBe("c");
	});

	it("only updates activeTopLevelCommand from depth-0 entries (nested delegates don't overwrite the top level)", () => {
		// Forward-compat with Stage 5: delegate-sourced entries will carry depth > 0
		// and must not be promoted to activeTopLevelCommand on replay.
		const entries = [cmdStart("top", 0), cmdStart("nested", 1), cmdStart("deeper", 2)];
		const result = replayHistory(entries);
		expect(activeCommandName(result.lifecycle)).toBe("top");
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
		expect(activeCommandName(result.lifecycle)).toBe("first");
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
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
		expect(activeCommandName(result.lifecycle)).toBe("a");
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
		expect(result.lifecycle.activeCommand).toBe("a");
	});

	it("reconstructs idle when a waiting command later completed without chaining (no resurrection)", () => {
		// The duplicate-work safety: a command that waited, was answered, then
		// completed without offering a next step writes no subsequent command-start,
		// so journaling the resolving status is what makes the rewind land on idle.
		// issue 128: completed clears activeTopLevelCommand so a later reply
		// doesn't re-arm the phase for a finished command.
		const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStatus("a", "completed")]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
		expect(derivedPhase(result.lifecycle)).toBe("idle");
	});

	it.each(["blocked", "incomplete"] as const)(
		"keeps command associated (dormant) when a waiting command later reported %s (issue 215)",
		(status) => {
			const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStatus("a", status)]);
			expect(derivedPhase(result.lifecycle)).toBe("dormant");
			expect(activeCommandName(result.lifecycle)).toBe("a");
		},
	);

	it("resets to dormant when a new depth-0 command starts after a waiting report", () => {
		// start(B) supersedes A: B has reported nothing yet, so its resting phase is
		// dormant (command associated but not actively running).
		const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStart("b")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("b");
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(result.lifecycle.activeCommand).toBe("b");
	});

	it("ignores a user-input-parked entry whose commandName does not match the active command", () => {
		// A stale entry from a since-superseded command must not move B's phase.
		const result = replayHistory([cmdStart("b"), userInputParked("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("b");
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
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("reconstructs waiting only from the LAST entry when several are journaled", () => {
		const result = replayHistory([cmdStart("a"), cmdStatus("a", "incomplete"), userInputParked("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
	});
});

// issue 352 (Stage 3): the unknown-slash workflow exit now emits a durable
// command-exited outcome, so replaying the actual emitted branch reconstructs
// idle. Pins the same-name identity contract the chronological fold relies on.
describe("replayHistory — issue 352 exit/identity characterization", () => {
	function toBranch(appended: { customType: string; data: unknown }[]): SessionEntry[] {
		return appended.map((a) => customEntry(a.customType, a.data));
	}

	it("unknown-slash exit replays idle, not dormant", async () => {
		const state = freshState({ registry: registryOf(["a:cmd"]) });
		const { pi, appended, emit } = recordingPi();
		// Known Pi commands exclude the typo, so it reads as a genuine unknown slash.
		(pi as any).getCommands = () => [{ name: "autopilot" }, { name: "clear" }];
		registerHistory(pi, state);

		// Real command start (journals a depth-0 command-start), then a true
		// unknown-slash exit (clears the active command live, journals the exit).
		await emit("input", { text: "/a:cmd", source: "interactive" });
		await emit("input", { text: "/typo-or-removed", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBeNull();

		// The emitted branch is the command-start followed by the durable exit.
		expect(appended.map((a) => a.customType)).toEqual([COMMAND_START_TYPE, COMMAND_EXIT_TYPE]);

		// Replaying the branch reconstructs idle: the exit supersedes the start.
		const replayed = replayHistory(toBranch(appended));
		expect(activeCommandName(replayed.lifecycle)).toBeNull();
		expect(derivedPhase(replayed.lifecycle)).toBe("idle");
	});

	it("a known Pi slash preserves the workflow and emits no exit (negative control)", async () => {
		const state = freshState({
			registry: registryOf(["a:cmd"]),
			lifecycle: lifecycleFor("dormant", "a:cmd"),
		});
		const { pi, appended, emit } = recordingPi();
		(pi as any).getCommands = () => [{ name: "autopilot" }];
		registerHistory(pi, state);

		await emit("input", { text: "/autopilot on", source: "interactive" });

		expect(activeCommandName(state.lifecycle)).toBe("a:cmd");
		expect(appended).toHaveLength(0);
	});

	// The command-name payloads carry no independent same-name invocation
	// identity: a depth-0 start resets the chronological fold, so a later
	// matching start re-associates the command regardless of prior outcomes.
	it("a later same-name start supersedes a prior park (dormant, not waiting)", () => {
		const result = replayHistory([cmdStart("a"), userInputParked("a"), cmdStart("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("a later same-name start re-associates after a completed report (dormant, not idle)", () => {
		const result = replayHistory([cmdStart("a"), cmdStatus("a", "completed"), cmdStart("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});
});

// issue 352 (Stage 3): the two durable outcomes added by this stage — the
// consumed-reply park (parked: false) and the command-exited outcome — folded
// chronologically over the selected branch.
describe("replayHistory — issue 352 durable outcome fold", () => {
	function parkedFlag(commandName: string, parked: unknown): SessionEntry {
		return customEntry(USER_INPUT_PARKED_TYPE, { commandName, parked });
	}
	function exited(commandName: string): SessionEntry {
		return customEntry(COMMAND_EXIT_TYPE, { commandName });
	}

	it("a consumed reply (parked: false) clears waiting to dormant", () => {
		const result = replayHistory([cmdStart("a"), parkedFlag("a", true), parkedFlag("a", false)]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("parked: false with no preceding park is inert (stays dormant)", () => {
		const result = replayHistory([cmdStart("a"), parkedFlag("a", false)]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("an explicit parked: true sets waiting", () => {
		const result = replayHistory([cmdStart("a"), parkedFlag("a", true)]);
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
	});

	it("a legacy park entry (no parked field) still means waiting", () => {
		const result = replayHistory([cmdStart("a"), userInputParked("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
	});

	it("a malformed parked value is inert (stays dormant)", () => {
		const result = replayHistory([cmdStart("a"), parkedFlag("a", "nope")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("a later park restores waiting after a consumed reply", () => {
		const result = replayHistory([
			cmdStart("a"),
			parkedFlag("a", true),
			parkedFlag("a", false),
			parkedFlag("a", true),
		]);
		expect(derivedPhase(result.lifecycle)).toBe("waiting");
	});

	it("a matching exit clears the command to idle", () => {
		const result = replayHistory([cmdStart("a"), exited("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
	});

	it("an exit clears a parked command to idle", () => {
		const result = replayHistory([cmdStart("a"), parkedFlag("a", true), exited("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
	});

	it("an exit naming a different command is inert", () => {
		const result = replayHistory([cmdStart("a"), exited("b")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("a malformed exit (missing commandName) is inert", () => {
		const result = replayHistory([cmdStart("a"), customEntry(COMMAND_EXIT_TYPE, {})]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("a later start re-associates after an exit (dormant, not idle)", () => {
		const result = replayHistory([cmdStart("a"), exited("a"), cmdStart("a")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});
});

describe("replayHistory — structured input cancellation", () => {
	const cancellation = (commandName: string, resumable: unknown) =>
		customEntry(STRUCTURED_INPUT_CANCELLATION_TYPE, { commandName, resumable });

	it("reconstructs cancellation-resumable dormancy from a matching grant", () => {
		const result = replayHistory([cmdStart("a"), cancellation("a", true)]);
		expect(result.lifecycle.cancellationResumeEligible).toBe(true);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
	});

	it("folds grant and consume chronologically", () => {
		const consumed = replayHistory([cmdStart("a"), cancellation("a", true), cancellation("a", false)]);
		expect(consumed.lifecycle.cancellationResumeEligible).toBe(false);
		const rewound = replayHistory([cmdStart("a"), cancellation("a", true)]);
		expect(rewound.lifecycle.cancellationResumeEligible).toBe(true);
	});

	it("ignores malformed and mismatched outcomes and resets on same-name restart", () => {
		const result = replayHistory([
			cmdStart("a"),
			cancellation("b", true),
			cancellation("a", "yes"),
			cancellation("a", true),
			cmdStart("a"),
		]);
		expect(result.lifecycle.cancellationResumeEligible).toBe(false);
	});

	it("terminal status, freetext park, and exit supersede a grant", () => {
		expect(
			replayHistory([cmdStart("a"), cancellation("a", true), cmdStatus("a", "blocked")]).lifecycle
				.cancellationResumeEligible,
		).toBe(false);
		expect(
			replayHistory([cmdStart("a"), cancellation("a", true), userInputParked("a")]).lifecycle
				.cancellationResumeEligible,
		).toBe(false);
		expect(
			replayHistory([cmdStart("a"), cancellation("a", true), customEntry(COMMAND_EXIT_TYPE, { commandName: "a" })])
				.lifecycle.cancellationResumeEligible,
		).toBe(false);
	});
});

describe("recordCommandStatus", () => {
	it("appends a COMMAND_STATUS_TYPE journal entry with the summary and mutates no state", () => {
		const { pi, appended } = recordingPi();
		recordCommandStatus(pi, "mach12:pr-create", "completed", "shipped stage 1");
		expect(appended).toHaveLength(1);
		expect(appended[0].customType).toBe(COMMAND_STATUS_TYPE);
		expect(appended[0].data as CommandStatusData).toEqual({
			commandName: "mach12:pr-create",
			status: "completed",
			summary: "shipped stage 1",
		});
	});

	it("serializes all four statuses, including continuing, with the summary", () => {
		for (const status of ["continuing", "completed", "blocked", "incomplete"] as const) {
			const { pi, appended } = recordingPi();
			recordCommandStatus(pi, "c", status, `did ${status}`);
			expect(appended[0].data as CommandStatusData).toEqual({
				commandName: "c",
				status,
				summary: `did ${status}`,
			});
		}
	});
});

describe("replayHistory — continuing is replay-inert (issue 278)", () => {
	it("reconstructs dormant from start → continuing (continuing never becomes a resting state)", () => {
		const result = replayHistory([
			cmdStart("a"),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "a", status: "continuing", summary: "more work" }),
		]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("a");
	});

	it("reconstructs idle from start → continuing → completed", () => {
		const result = replayHistory([
			cmdStart("a"),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "a", status: "continuing", summary: "progress" }),
			cmdStatus("a", "completed"),
		]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
	});

	it("replays a legacy terminal entry that has no summary field", () => {
		const result = replayHistory([
			cmdStart("a"),
			customEntry(COMMAND_STATUS_TYPE, { commandName: "a", status: "completed" }),
		]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
	});
});

describe("recordCommandInvocation", () => {
	it("records depth-0 starts as active top-level commands", () => {
		const state = freshState();
		const { pi, appended } = recordingPi();

		recordCommandInvocation(pi, state, "top", "user", 0);

		expect(activeCommandName(state.lifecycle)).toBe("top");
		expect(state.sidebarLog[0]).toMatchObject({ command: "top", origin: "user", depth: 0 });
		expect((appended[0].data as SidebarEntry).depth).toBe(0);
	});

	it("records delegated depth entries without replacing the lifecycle command", () => {
		const state = freshState({ lifecycle: lifecycleFor("dormant", "top") });
		const { pi, appended } = recordingPi();

		recordCommandInvocation(pi, state, "delegate", "agent", 1);

		expect(activeCommandName(state.lifecycle)).toBe("top");
		expect(state.sidebarLog[0]).toMatchObject({ command: "delegate", origin: "agent", depth: 1 });
		expect(appended[0].customType).toBe(COMMAND_START_TYPE);
		expect(appended[0].data as SidebarEntry).toMatchObject({ command: "delegate", origin: "agent", depth: 1 });
	});

	it("starts a depth-0 command in the running phase and clears any prior status (issue 84)", () => {
		const state = freshState({
			lifecycle: lifecycleFor("reported", "old"),
		});
		const { pi } = recordingPi();

		recordCommandInvocation(pi, state, "top", "user", 0);

		expect(derivedPhase(state.lifecycle)).toBe("running");
		expect(derivedPhase(state.lifecycle)).toBe("running");
		expect(state.lifecycle.activeCommand).toBe("top");
	});

	it("does not touch the command phase for delegated depth entries (probe stays probing)", () => {
		// The probe turn is not a command start; a delegate during it must not
		// reset the phase, or the status report would be rejected as out-of-phase.
		const state = freshState({ lifecycle: lifecycleFor("probing", "top") });
		const { pi } = recordingPi();

		recordCommandInvocation(pi, state, "delegate", "agent", 1);

		expect(derivedPhase(state.lifecycle)).toBe("probing");
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

	it("advances generation whenever reconstruction replaces lifecycle state", async () => {
		const state = freshState({ lifecycleGeneration: 7 });
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("session_start", {}, ctxWithEntries([]));
		expect(state.lifecycleGeneration).toBe(8);
		await emit("session_tree", {}, ctxWithEntries([]));
		expect(state.lifecycleGeneration).toBe(9);
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
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
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
			lifecycle: lifecycleFor("dormant", "mach10:push"),
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "/typo-or-removed", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBeNull();
	});

	it("does NOT clear activeTopLevelCommand for known Pi/autopilot built-in slash commands (F4)", async () => {
		// /autopilot on, /clear, /help etc. are registered with Pi, not the
		// command-set registry. The user toggling /autopilot on mid-workflow
		// must not silently break the forced chain.
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "mach10:push"),
		});
		const { pi, emit } = recordingPi();
		// Simulate pi.getCommands() returning known commands.
		(pi as any).getCommands = () => [
			{ name: "autopilot", description: "toggle", source: "extension", sourceInfo: {} },
			{ name: "clear", description: "clear", source: "extension", sourceInfo: {} },
		];
		registerHistory(pi, state);
		await emit("input", { text: "/autopilot on", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
		await emit("input", { text: "/clear", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
	});

	it("falls back to allow-list when pi.getCommands is unavailable (F4)", async () => {
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "mach10:push"),
		});
		const { pi, emit } = recordingPi();
		// No getCommands on the fake pi — tests fallback allow-list path.
		registerHistory(pi, state);
		await emit("input", { text: "/autopilot on", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
		await emit("input", { text: "/clear", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
		// Removed/internal or unknown slashes are not allow-listed.
		await emit("input", { text: "/scramjet-exec-fresh foo", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBeNull();
	});

	it("logs and preserves the active workflow when pi.getCommands throws", async () => {
		const logger = { warn: vi.fn(), debug: vi.fn(), lifecycle: vi.fn() };
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "mach10:push"),
			logger: logger as any,
		});
		const { pi, emit } = recordingPi();
		(pi as any).getCommands = () => {
			throw new Error("registry unavailable");
		};
		registerHistory(pi, state);

		await emit("input", { text: "/other-extension:cmd", source: "interactive" });

		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
		expect(logger.warn).toHaveBeenCalledWith(
			"history",
			"slash command lookup failed; preserving active Scramjet workflow",
			expect.objectContaining({ slashName: "other-extension:cmd", error: "registry unavailable" }),
		);
	});

	it("leaves activeTopLevelCommand alone for non-slash input (continuing a conversation)", async () => {
		// Plain follow-up text must not nuke the active workflow — otherwise
		// any chat after the command would disable next-step auto-continue.
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "mach10:push"),
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "plain follow-up", source: "interactive" });
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
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
			lifecycle: lifecycleFor("dormant", "mach10:push"),
		});
		const { pi, emit } = recordingPi();
		(pi as any).getCommands = () => [{ name: "other-extension:cmd" }];
		registerHistory(pi, state);

		await emit("input", { text: "/other-extension:cmd --flag", source: "extension" });

		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
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
				lifecycle: lifecycleFor("waiting", "mach12:pr-create"),
			});
		}

		it("flips waiting→running on an interactive non-slash reply", async () => {
			const state = waitingState();
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "approve", source: "interactive" });
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(activeCommandName(state.lifecycle)).toBe("mach12:pr-create");
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(state.lifecycle.activeCommand).toBe("mach12:pr-create");
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
			expect(derivedPhase(state.lifecycle)).toBe("waiting");
		});

		it("treats a registered slash command while waiting as a normal command start", async () => {
			const state = waitingState();
			const { pi, appended, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "/mach12:pr-create", source: "interactive" });
			// recordCommandStart fires: phase running, active set, journaled.
			expect(derivedPhase(state.lifecycle)).toBe("running");
			expect(activeCommandName(state.lifecycle)).toBe("mach12:pr-create");
			expect(appended).toHaveLength(1);
		});

		it("drops the waiting phase to idle when an unknown slash exits the workflow (F25)", async () => {
			const state = waitingState();
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "/typo-or-removed", source: "interactive" });
			expect(activeCommandName(state.lifecycle)).toBeNull();
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("does NOT auto-resume dormant on interactive non-slash reply (issue 215: agent-controlled resumption)", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: lifecycleFor("dormant", "mach12:pr-create"),
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "just chatting", source: "interactive" });
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(activeCommandName(state.lifecycle)).toBe("mach12:pr-create");
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(state.lifecycle.activeCommand).toBe("mach12:pr-create");
			expect(state.sidebarLog).toHaveLength(0);
		});

		it("does not re-arm idle→running when lifecycle is idle", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: lifecycleFor("idle"),
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "just chatting", source: "interactive" });
			expect(derivedPhase(state.lifecycle)).toBe("idle");
			expect(derivedPhase(state.lifecycle)).toBe("idle");
		});

		it("does not re-arm dormant→running on extension-source replies", async () => {
			const state = freshState({
				registry: registryOf(["mach12:pr-create"]),
				lifecycle: lifecycleFor("dormant", "mach12:pr-create"),
			});
			const { pi, emit } = recordingPi();
			registerHistory(pi, state);
			await emit("input", { text: "approve", source: "extension" });
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(derivedPhase(state.lifecycle)).toBe("dormant");
			expect(state.lifecycle.activeCommand).toBe("mach12:pr-create");
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
		expect(activeCommandName(state.lifecycle)).toBe("b");
	});

	it("rebuilds state on session_tree the same way as session_start", async () => {
		const { state, emit, ctx } = setup([cmdStart("only")]);
		await emit("session_tree", {}, ctx);
		expect(state.sidebarLog.map((e) => e.command)).toEqual(["only"]);
		expect(activeCommandName(state.lifecycle)).toBe("only");
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
			lifecycle: lifecycleFor("probing", "stale"),
		});
		await emit("session_start", {}, ctx);
		// Replay reconstructs to dormant (command-start "a" present, no terminal status).
		expect(derivedPhase(state.lifecycle)).toBe("dormant");
	});

	// issue 88 / issue 156: a paused command survives rewind/resume. The
	// journaled user-input-parked entry reconstructs the stable "waiting" phase
	// so a later interactive reply can resume the command.
	it("reconstructs the waiting phase on rebuild when the active command has a user-input-parked entry", async () => {
		const { state, emit, ctx } = setup([cmdStart("a"), userInputParked("a")], {
			lifecycle: lifecycleFor("probing", "stale"),
		});
		await emit("session_start", {}, ctx);
		expect(derivedPhase(state.lifecycle)).toBe("waiting");
		expect(activeCommandName(state.lifecycle)).toBe("a");
	});

	it("reconstructs idle on rebuild when a waiting command later completed (no resurrection)", async () => {
		const { state, emit, ctx } = setup([cmdStart("a"), userInputParked("a"), cmdStatus("a", "completed")], {
			lifecycle: lifecycleFor("running", "stale"),
		});
		await emit("session_start", {}, ctx);
		expect(derivedPhase(state.lifecycle)).toBe("idle");
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

describe("replayHistory — blocked/incomplete keep command associated (issue 215)", () => {
	it.each(["blocked", "incomplete"] as const)(
		"reconstructs dormant (not idle) when active command reported %s",
		(status) => {
			const result = replayHistory([cmdStart("a"), cmdStatus("a", status)]);
			expect(derivedPhase(result.lifecycle)).toBe("dormant");
			expect(activeCommandName(result.lifecycle)).toBe("a");
		},
	);

	it("reconstructs idle when active command reported completed", () => {
		const result = replayHistory([cmdStart("a"), cmdStatus("a", "completed")]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
	});

	it("reconstructs dormant when blocked is followed by a new command start", () => {
		const result = replayHistory([cmdStart("a"), cmdStatus("a", "blocked"), cmdStart("b")]);
		expect(derivedPhase(result.lifecycle)).toBe("dormant");
		expect(activeCommandName(result.lifecycle)).toBe("b");
	});

	it("reconstructs idle when blocked command is followed by a completed command", () => {
		const result = replayHistory([
			cmdStart("a"),
			cmdStatus("a", "blocked"),
			cmdStart("b"),
			cmdStatus("b", "completed"),
		]);
		expect(derivedPhase(result.lifecycle)).toBe("idle");
		expect(activeCommandName(result.lifecycle)).toBeNull();
	});
});

describe("timer cleanup wiring (issue 215)", () => {
	it("calls clearLifecycleTimers before depth-0 command-start transition", () => {
		const calls: string[] = [];
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "old"),
			clearLifecycleTimers: () => calls.push("cleared"),
		});
		const { pi } = recordingPi();
		recordCommandInvocation(pi, state, "mach10:push", "user", 0);
		expect(calls).toEqual(["cleared"]);
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
	});

	it("does not call clearLifecycleTimers for delegated (depth > 0) invocations", () => {
		const calls: string[] = [];
		const state = freshState({
			lifecycle: lifecycleFor("running", "top"),
			clearLifecycleTimers: () => calls.push("cleared"),
		});
		const { pi } = recordingPi();
		recordCommandInvocation(pi, state, "delegate", "agent", 1);
		expect(calls).toEqual([]);
	});

	it("calls clearLifecycleTimers on unknown-slash workflow exit", async () => {
		const calls: string[] = [];
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "mach10:push"),
			clearLifecycleTimers: () => calls.push("cleared"),
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("input", { text: "/typo-or-removed", source: "interactive" });
		expect(calls).toEqual(["cleared"]);
		expect(activeCommandName(state.lifecycle)).toBeNull();
	});

	it("calls clearLifecycleTimers on session rebuild", async () => {
		const calls: string[] = [];
		const state = freshState({
			lifecycle: lifecycleFor("running", "stale"),
			clearLifecycleTimers: () => calls.push("cleared"),
		});
		const { pi, emit } = recordingPi();
		registerHistory(pi, state);
		await emit("session_start", {}, ctxWithEntries([cmdStart("a")]));
		expect(calls).toEqual(["cleared"]);
	});

	it("does not call clearLifecycleTimers when it is not set", () => {
		const state = freshState({
			registry: registryOf(["mach10:push"]),
			lifecycle: lifecycleFor("dormant", "old"),
		});
		const { pi } = recordingPi();
		// Should not throw when clearLifecycleTimers is undefined
		recordCommandInvocation(pi, state, "mach10:push", "user", 0);
		expect(activeCommandName(state.lifecycle)).toBe("mach10:push");
	});
});
