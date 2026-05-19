import type { SessionEntry } from "@earendil-works/pi-coding-agent";
import { describe, expect, it } from "vitest";
import {
	appendSidebarEntry,
	COMMAND_START_TYPE,
	ENABLED_TOGGLE_TYPE,
	parseSlashCommand,
	registerHistory,
	replayHistory,
	SIDEBAR_MAX,
} from "../history.ts";
import type { CommandDef, CommandRegistry, ScramjetState, SidebarEntry } from "../types.ts";

function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
		enabled: false,
		registry: new Map(),
		activeTopLevelCommand: null,
		sidebarLog: [],
		delegateStack: [],
		...overrides,
	};
}

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
		expect(result.activeTopLevelCommand).toBeNull();
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
		expect(result.activeTopLevelCommand).toBe("c");
	});

	it("only updates activeTopLevelCommand from depth-0 entries (nested delegates don't overwrite the top level)", () => {
		// Forward-compat with Stage 5: delegate-sourced entries will carry depth > 0
		// and must not be promoted to activeTopLevelCommand on replay.
		const entries = [cmdStart("top", 0), cmdStart("nested", 1), cmdStart("deeper", 2)];
		const result = replayHistory(entries);
		expect(result.activeTopLevelCommand).toBe("top");
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

	it("trims a replayed log of more than SIDEBAR_MAX entries", () => {
		const entries: SessionEntry[] = [];
		for (let i = 0; i < SIDEBAR_MAX + 5; i++) entries.push(cmdStart(`c-${i}`));
		const result = replayHistory(entries);
		expect(result.sidebarLog).toHaveLength(SIDEBAR_MAX);
		expect(result.sidebarLog[0].command).toBe("c-5");
	});
});

describe("registerHistory — handler registration", () => {
	it("registers handlers for input, session_start, and session_tree", () => {
		const { pi, handlers } = recordingPi();
		registerHistory(pi, freshState());
		expect(handlers.get("input")).toHaveLength(1);
		expect(handlers.get("session_start")).toHaveLength(1);
		expect(handlers.get("session_tree")).toHaveLength(1);
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
		expect(state.activeTopLevelCommand).toBe("mach10:push");
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
		expect(state.activeTopLevelCommand).toBe("b");
	});

	it("rebuilds state on session_tree the same way as session_start", async () => {
		const { state, emit, ctx } = setup([cmdStart("only")]);
		await emit("session_tree", {}, ctx);
		expect(state.sidebarLog.map((e) => e.command)).toEqual(["only"]);
		expect(state.activeTopLevelCommand).toBe("only");
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
});
