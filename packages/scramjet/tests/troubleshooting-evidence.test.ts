import type { SessionEntry } from "@leanandmean/coding-agent";
import { Value } from "typebox/value";
import { describe, expect, it } from "vitest";
import { COMMAND_START_TYPE, COMMAND_STATUS_TYPE } from "../src/history.js";
import { initScramjet } from "../src/index.js";
import {
	ERROR_MESSAGES,
	MAX_BRANCH_ANCESTRY,
	MAX_SESSION_ENTRIES,
	registerTroubleshootingEvidenceTool,
} from "../src/troubleshooting-evidence.js";
import { freshState, lifecycleFor, recordingPi } from "./helpers.js";

const IDS = Array.from({ length: 80 }, (_, index) => index.toString(16).padStart(8, "0"));

function custom(id: string, parentId: string | null, customType: string, data: unknown): SessionEntry {
	return { type: "custom", id, parentId, timestamp: "2026-07-23T00:00:00.000Z", customType, data };
}

function commandStart(id: string, parentId: string | null, command: string, depth = 0): SessionEntry {
	return custom(id, parentId, COMMAND_START_TYPE, {
		command,
		origin: "user",
		depth,
		timestamp: 1,
	});
}

function message(
	id: string,
	parentId: string | null,
	role: "user" | "assistant" = "assistant",
	provider = "openai",
	model = "execution-model",
): SessionEntry {
	return {
		type: "message",
		id,
		parentId,
		timestamp: "2026-07-23T00:00:00.000Z",
		message: { role, content: [], provider, model } as any,
	};
}

function linear(...entries: SessionEntry[]): SessionEntry[] {
	return entries;
}

function validEntries(): SessionEntry[] {
	return linear(
		commandStart(IDS[0], null, "mach12:issue-plan"),
		message(IDS[1], IDS[0]),
		custom(IDS[2], IDS[1], COMMAND_STATUS_TYPE, {
			commandName: "mach12:issue-plan",
			status: "blocked",
			summary: "blocked",
		}),
		commandStart(IDS[3], IDS[2], "scramjet:troubleshoot"),
		message(IDS[4], IDS[3]),
	);
}

function harness(entries = validEntries(), options: { leafId?: string | null; sessionId?: string; cwd?: string } = {}) {
	let currentEntries = entries;
	let leafId = options.leafId === undefined ? (entries.at(-1)?.id ?? null) : options.leafId;
	let sessionId = options.sessionId ?? "019c0000-0000-7000-8000-000000000001";
	let getBranchCalls = 0;
	let getEntriesCalls = 0;
	let getEntryOverride: ((id: string) => SessionEntry | undefined) | null = null;
	const manager = {
		getSessionId: () => sessionId,
		getLeafId: () => leafId,
		getEntries: () => {
			getEntriesCalls++;
			return currentEntries;
		},
		getEntry: (id: string) =>
			getEntryOverride ? getEntryOverride(id) : currentEntries.find((entry) => entry.id === id),
		getHeader: () => ({
			type: "session",
			id: sessionId,
			timestamp: "2026-07-23T00:00:00.000Z",
			cwd: options.cwd ?? "/repo",
		}),
		getBranch: () => {
			getBranchCalls++;
			throw new Error("getBranch must not be called");
		},
	};
	const state = freshState({ lifecycle: lifecycleFor("running", "scramjet:troubleshoot") });
	const recording = recordingPi();
	registerTroubleshootingEvidenceTool(recording.pi, state);
	const tool = recording.tools.find((candidate) => candidate.name === "get_scramjet_troubleshooting_evidence");
	const context = {
		sessionManager: manager,
		cwd: "/repo",
		model: { provider: "anthropic", id: "troubleshooting-model", name: "Troubleshooting Model" },
	};
	const execute = async (params: unknown) => {
		const result = await tool.execute("tool-call", params, undefined, undefined, context);
		return result.details as any;
	};
	return {
		state,
		recording,
		tool,
		execute,
		setEntries(next: SessionEntry[], nextLeaf = next.at(-1)?.id ?? null) {
			currentEntries = next;
			leafId = nextLeaf;
		},
		setSessionId(next: string) {
			sessionId = next;
		},
		setGetEntryOverride(next: ((id: string) => SessionEntry | undefined) | null) {
			getEntryOverride = next;
		},
		getBranchCalls: () => getBranchCalls,
		getEntriesCalls: () => getEntriesCalls,
	};
}

const RETRYABLE_ERRORS = new Set<keyof typeof ERROR_MESSAGES>([
	"SNAPSHOT_NOT_FOUND",
	"SNAPSHOT_SESSION_MISMATCH",
	"SNAPSHOT_BRANCH_CHANGED",
	"CURSOR_NOT_FOUND",
	"CURSOR_MISMATCH",
	"INTERNAL_ERROR",
]);

function expectError(result: any, code: keyof typeof ERROR_MESSAGES) {
	expect(result).toEqual({
		schema: "scramjet.troubleshooting-evidence/v1",
		ok: false,
		code,
		message: ERROR_MESSAGES[code],
		retryable: RETRYABLE_ERRORS.has(code),
	});
}

async function openAndSelect(h: ReturnType<typeof harness>, candidateIndex = 0) {
	const opened = await h.execute({ action: "open" });
	expect(opened.ok).toBe(true);
	const target = opened.data.candidates[candidateIndex];
	const selected = await h.execute({
		action: "select",
		snapshot_id: opened.snapshot_id,
		target_ref: target.target_ref,
	});
	return { opened, target, selected };
}

describe("troubleshooting evidence protocol", () => {
	it("registers one model-callable sequential tool from the product entry point", () => {
		const direct = harness();
		expect(direct.tool).toMatchObject({
			name: "get_scramjet_troubleshooting_evidence",
			executionMode: "sequential",
		});
		expect(direct.tool.activation).not.toBe("harness-only");
		expect(direct.tool.promptSnippet).toContain("open → select → index/read");

		const recording = recordingPi();
		initScramjet(recording.pi);
		expect(recording.tools.filter((tool) => tool.name === direct.tool.name)).toHaveLength(1);
	});

	it("normalizes every malformed argument shape to a reserved valid sentinel", async () => {
		const h = harness();
		const hostile = "SECRET-HOSTILE-ARGUMENT";
		for (const args of [
			null,
			{ action: "unknown", secret: hostile },
			{ action: "open", extra: hostile },
			{ action: "select", snapshot_id: hostile, target_ref: hostile },
			{ action: "index", evidence_refs: [hostile] },
			{ action: "read", evidence_refs: [] },
			{ action: "read", cursor: hostile, evidence_refs: [hostile] },
		]) {
			const prepared = h.tool.prepareArguments(args);
			expect(Value.Check(h.tool.parameters, prepared)).toBe(true);
			const result = await h.execute(prepared);
			expectError(result, "INVALID_ARGUMENT");
			expect(JSON.stringify(result)).not.toContain(hostile);
		}
	});

	it("gates every action to the active troubleshoot command before session access", async () => {
		const h = harness();
		for (const lifecycle of [lifecycleFor("idle"), lifecycleFor("running", "mach12:issue-plan")]) {
			h.state.lifecycle = lifecycle;
			expectError(await h.execute({ action: "open" }), "COMMAND_NOT_ACTIVE");
		}
		expect(h.getEntriesCalls()).toBe(0);
	});

	it("returns exact success envelopes and opaque domain-separated references", async () => {
		const h = harness();
		const { opened, target, selected } = await openAndSelect(h);
		expect(Object.keys(opened).sort()).toEqual(["action", "data", "gaps", "ok", "schema", "snapshot_id"]);
		expect(Object.keys(opened.data).sort()).toEqual([
			"candidates",
			"cwd_relation",
			"proposed_target_ref",
			"session_ref",
		]);
		expect(opened).toMatchObject({
			schema: "scramjet.troubleshooting-evidence/v1",
			ok: true,
			action: "open",
			snapshot_id: expect.stringMatching(/^snp-v1-[a-z2-7]{26}$/),
			data: {
				session_ref: expect.stringMatching(/^ses-v1-[a-z2-7]{26}$/),
				proposed_target_ref: expect.stringMatching(/^inv-v1-[a-z2-7]{26}$/),
			},
			gaps: [],
		});
		expect(target.target_ref).toMatch(/^inv-v1-[a-z2-7]{26}$/);
		expect(Object.keys(selected).sort()).toEqual(["action", "data", "gaps", "ok", "schema", "snapshot_id"]);
		expect(Object.keys(selected.data).sort()).toEqual([
			"command",
			"current_source_available",
			"cwd_relation",
			"execution_models",
			"handoff_id",
			"relation",
			"target_ref",
			"terminal_status",
			"troubleshooting_model",
		]);
		expect(selected).toMatchObject({
			schema: "scramjet.troubleshooting-evidence/v1",
			ok: true,
			action: "select",
			snapshot_id: opened.snapshot_id,
			data: {
				handoff_id: expect.stringMatching(/^sth-v1-[a-z2-7]{26}$/),
				target_ref: target.target_ref,
			},
			gaps: [],
		});
		const serialized = JSON.stringify({ opened, selected });
		for (const raw of [...IDS.slice(0, 5), "019c0000-0000-7000-8000-000000000001"]) {
			expect(serialized).not.toContain(raw);
		}
	});
});

describe("bounded ancestry validation", () => {
	it("rejects an absent current branch", async () => {
		const h = harness([], { leafId: null });
		expectError(await h.execute({ action: "open" }), "NO_CURRENT_BRANCH");
	});

	it("rejects the total session-entry bound before traversing", async () => {
		const repeated = Array.from({ length: MAX_SESSION_ENTRIES + 1 }, () => validEntries()[0]);
		const h = harness(repeated);
		expectError(await h.execute({ action: "open" }), "SESSION_ENTRY_LIMIT");
	});

	it.each([
		["SESSION_INVALID_ENTRY_ID", [commandStart("BAD", null, "x:y")]],
		["SESSION_INVALID_PARENT_ID", [commandStart(IDS[0], "BAD", "x:y")]],
		["SESSION_DUPLICATE_ENTRY_ID", [commandStart(IDS[0], null, "x:y"), message(IDS[0], IDS[0])]],
		["SESSION_ROOT_MISSING", [commandStart(IDS[0], IDS[1], "x:y"), message(IDS[1], IDS[0])]],
		["SESSION_MULTIPLE_ROOTS", [commandStart(IDS[0], null, "x:y"), message(IDS[1], null)]],
		["SESSION_SELF_CYCLE", [commandStart(IDS[0], IDS[0], "x:y")]],
		["SESSION_BROKEN_PARENT", [commandStart(IDS[0], null, "x:y"), message(IDS[1], IDS[2])]],
	] as const)("returns %s for malformed metadata", async (code, entries) => {
		const h = harness(entries as SessionEntry[]);
		expectError(await h.execute({ action: "open" }), code);
	});

	it("detects an off-branch cycle without inspecting sibling content", async () => {
		let siblingContentReads = 0;
		const siblingA = new Proxy(message(IDS[10], IDS[11]) as any, {
			get(target, key, receiver) {
				if (key === "message") siblingContentReads++;
				return Reflect.get(target, key, receiver);
			},
		});
		const siblingB = new Proxy(message(IDS[11], IDS[10]) as any, {
			get(target, key, receiver) {
				if (key === "message") siblingContentReads++;
				return Reflect.get(target, key, receiver);
			},
		});
		const h = harness([...validEntries(), siblingA, siblingB]);
		expectError(await h.execute({ action: "open" }), "SESSION_CYCLE");
		expect(siblingContentReads).toBe(0);
	});

	it("walks only through getEntry, reverses root-to-leaf, and never calls getBranch", async () => {
		const h = harness();
		const opened = await h.execute({ action: "open" });
		expect(opened.ok).toBe(true);
		expect(opened.data.candidates.map((candidate: any) => candidate.command)).toEqual(["mach12:issue-plan"]);
		expect(h.getBranchCalls()).toBe(0);
	});

	it("rejects missing or structurally inconsistent getEntry results", async () => {
		const h = harness();
		h.setGetEntryOverride(() => undefined);
		expectError(await h.execute({ action: "open" }), "SESSION_BROKEN_PARENT");

		const entries = validEntries();
		h.setGetEntryOverride((id) => {
			const entry = entries.find((candidate) => candidate.id === id);
			return entry ? ({ ...entry, parentId: null } as SessionEntry) : undefined;
		});
		expectError(await h.execute({ action: "open" }), "SESSION_BROKEN_PARENT");
	});

	it("enforces the selected ancestry bound", async () => {
		const entries: SessionEntry[] = [];
		for (let index = 0; index <= MAX_BRANCH_ANCESTRY; index++) {
			const id = index.toString(16).padStart(8, "0");
			entries.push(message(id, index === 0 ? null : (index - 1).toString(16).padStart(8, "0")));
		}
		const h = harness(entries);
		expectError(await h.execute({ action: "open" }), "SESSION_ANCESTRY_LIMIT");
	});
});

describe("targeting and immutable snapshots", () => {
	function targetingEntries(): SessionEntry[] {
		return [
			commandStart(IDS[0], null, "first:command"),
			message(IDS[1], IDS[0], "assistant", "openai", "first-model"),
			commandStart(IDS[2], IDS[1], "delegate:helper", 1),
			message(IDS[3], IDS[2]),
			commandStart(IDS[4], IDS[3], "scramjet:troubleshoot"),
			message(IDS[5], IDS[4]),
			commandStart(IDS[6], IDS[5], "second:command"),
			message(IDS[7], IDS[6], "assistant", "anthropic", "second-model"),
			commandStart(IDS[8], IDS[7], "scramjet:troubleshoot"),
			message(IDS[9], IDS[8]),
		];
	}

	it("proposes the nearest non-troubleshoot target, preserves older targets, and ignores delegates", async () => {
		const h = harness(targetingEntries());
		const opened = await h.execute({ action: "open" });
		expect(opened.data.candidates.map((candidate: any) => [candidate.command, candidate.relation])).toEqual([
			["second:command", "nearest-non-troubleshoot"],
			["scramjet:troubleshoot", "prior-troubleshoot"],
			["first:command", "older"],
		]);
		expect(opened.data.candidates.some((candidate: any) => candidate.command === "delegate:helper")).toBe(false);
	});

	it("retains the nearest non-troubleshoot proposal under the 20-candidate bound", async () => {
		const entries: SessionEntry[] = [commandStart(IDS[0], null, "first:command"), message(IDS[1], IDS[0])];
		let parentId = IDS[1];
		for (let index = 0; index < 20; index++) {
			const startId = IDS[index * 2 + 2] as string;
			const messageId = IDS[index * 2 + 3] as string;
			entries.push(commandStart(startId, parentId, "scramjet:troubleshoot"), message(messageId, startId));
			parentId = messageId;
		}
		const finalStart = IDS[42] as string;
		entries.push(commandStart(finalStart, parentId, "scramjet:troubleshoot"), message(IDS[43] as string, finalStart));
		const h = harness(entries);
		const opened = await h.execute({ action: "open" });
		expect(opened.data.candidates).toHaveLength(20);
		expect(opened.data.proposed_target_ref).toMatch(/^inv-v1-/);
		expect(opened.data.candidates.some((candidate: any) => candidate.command === "first:command")).toBe(true);
	});

	it("keeps unsafe command, provider, and model pseudonyms in separate domains", async () => {
		const entries = [
			commandStart(IDS[0], null, "@unsafe"),
			message(IDS[1], IDS[0], "assistant", "@unsafe", "@unsafe"),
			commandStart(IDS[2], IDS[1], "scramjet:troubleshoot"),
			message(IDS[3], IDS[2]),
		];
		const h = harness(entries);
		const { opened, selected } = await openAndSelect(h);
		expect(opened.data.candidates[0].command).toMatch(/^cmd-v1-/);
		expect(selected.data.execution_models[0].provider).toMatch(/^prv-v1-/);
		expect(selected.data.execution_models[0].model).toMatch(/^mdl-v1-/);
	});

	it("allows an explicitly selected prior troubleshoot when no non-troubleshoot target exists", async () => {
		const entries = [
			commandStart(IDS[0], null, "scramjet:troubleshoot"),
			message(IDS[1], IDS[0]),
			commandStart(IDS[2], IDS[1], "scramjet:troubleshoot"),
			message(IDS[3], IDS[2]),
		];
		const h = harness(entries);
		const opened = await h.execute({ action: "open" });
		expect(opened.data.proposed_target_ref).toBeNull();
		expect(opened.data.candidates).toHaveLength(1);
		const selected = await h.execute({
			action: "select",
			snapshot_id: opened.snapshot_id,
			target_ref: opened.data.candidates[0].target_ref,
		});
		expect(selected.ok).toBe(true);
	});

	it("selects only issued target references and reports separate execution/troubleshooting models", async () => {
		const h = harness(targetingEntries());
		const opened = await h.execute({ action: "open" });
		expectError(
			await h.execute({ action: "select", snapshot_id: opened.snapshot_id, target_ref: IDS[6] }),
			"INVALID_ARGUMENT",
		);
		expectError(
			await h.execute({
				action: "select",
				snapshot_id: opened.snapshot_id,
				target_ref: "inv-v1-bbbbbbbbbbbbbbbbbbbbbbbbbb",
			}),
			"UNKNOWN_REFERENCE",
		);
		const selected = await h.execute({
			action: "select",
			snapshot_id: opened.snapshot_id,
			target_ref: opened.data.proposed_target_ref,
		});
		expect(selected.data.execution_models).toEqual([{ provider: "anthropic", model: "second-model" }]);
		expect(selected.data.troubleshooting_model).toEqual({ provider: "anthropic", model: "troubleshooting-model" });
		expect(selected.data.cwd_relation).toBe("match");
	});

	it("rejects target references issued by another snapshot", async () => {
		const h = harness(targetingEntries());
		const first = await h.execute({ action: "open" });
		const second = await h.execute({ action: "open" });
		expectError(
			await h.execute({
				action: "select",
				snapshot_id: second.snapshot_id,
				target_ref: first.data.proposed_target_ref,
			}),
			"UNKNOWN_REFERENCE",
		);
	});

	it("locks a snapshot to its first selected target", async () => {
		const h = harness(targetingEntries());
		const opened = await h.execute({ action: "open" });
		const first = opened.data.candidates[0];
		const second = opened.data.candidates[1];
		expect(
			(
				await h.execute({
					action: "select",
					snapshot_id: opened.snapshot_id,
					target_ref: first.target_ref,
				})
			).ok,
		).toBe(true);
		expectError(
			await h.execute({
				action: "select",
				snapshot_id: opened.snapshot_id,
				target_ref: second.target_ref,
			}),
			"TARGET_OUTSIDE_SNAPSHOT",
		);
	});

	it("accepts appends but rejects branch, session, and lifecycle changes on continuations", async () => {
		const entries = validEntries();
		const h = harness(entries);
		const { opened } = await openAndSelect(h);
		const appended = [...entries, message(IDS[5], IDS[4])];
		h.setEntries(appended);
		const stable = await h.execute({ action: "index", snapshot_id: opened.snapshot_id });
		expect(stable.ok || stable.code === "RESOURCE_LIMIT").toBe(true);

		const branch = [...entries, message(IDS[6], IDS[3])];
		h.setEntries(branch);
		expectError(await h.execute({ action: "index", snapshot_id: opened.snapshot_id }), "SNAPSHOT_BRANCH_CHANGED");

		const corrupt = appended.map((entry) => ({ ...entry })) as SessionEntry[];
		corrupt[1] = { ...(corrupt[1] as SessionEntry), parentId: IDS[20] } as SessionEntry;
		h.setEntries(corrupt);
		expectError(await h.execute({ action: "index", snapshot_id: opened.snapshot_id }), "SESSION_BROKEN_PARENT");

		h.setEntries(appended);
		h.setSessionId("019c0000-0000-7000-8000-000000000002");
		expectError(await h.execute({ action: "index", snapshot_id: opened.snapshot_id }), "SNAPSHOT_SESSION_MISMATCH");

		h.setSessionId("019c0000-0000-7000-8000-000000000001");
		h.state.lifecycle = lifecycleFor("running", "mach12:issue-plan");
		expectError(await h.execute({ action: "index", snapshot_id: opened.snapshot_id }), "COMMAND_NOT_ACTIVE");
	});

	it("clears snapshots on session rebuild events", async () => {
		const h = harness();
		const opened = await h.execute({ action: "open" });
		await h.recording.emit("session_tree");
		expectError(
			await h.execute({
				action: "select",
				snapshot_id: opened.snapshot_id,
				target_ref: opened.data.proposed_target_ref,
			}),
			"SNAPSHOT_NOT_FOUND",
		);
	});

	it("refuses a troubleshoot invocation with no earlier selectable target", async () => {
		const entries = [commandStart(IDS[0], null, "scramjet:troubleshoot"), message(IDS[1], IDS[0])];
		const h = harness(entries);
		expectError(await h.execute({ action: "open" }), "NO_TROUBLESHOOT_INVOCATION");
	});
});
