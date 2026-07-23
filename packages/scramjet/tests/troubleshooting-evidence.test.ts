import type { SessionEntry } from "@leanandmean/coding-agent";
import { Value } from "typebox/value";
import { describe, expect, it } from "vitest";
import { COMMAND_START_TYPE, COMMAND_STATUS_TYPE } from "../src/history.js";
import { initScramjet } from "../src/index.js";
import {
	ERROR_MESSAGES,
	EVIDENCE_GAPS,
	MAX_BRANCH_ANCESTRY,
	MAX_MESSAGE_CONTENT_BLOCKS,
	MAX_SAFE_JSON_ITEMS,
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
		setModel(provider: string, id: string) {
			context.model = { provider, id, name: id };
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
			"authoring_guide",
			"command",
			"current_source",
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
			gaps: [EVIDENCE_GAPS.SOURCE_UNAVAILABLE],
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

function evidenceEntries(argumentsValue: unknown): SessionEntry[] {
	return [
		commandStart(IDS[0], null, "test:target"),
		{
			type: "message",
			id: IDS[1],
			parentId: IDS[0],
			timestamp: "2026-07-23T00:00:00.000Z",
			message: { role: "user", content: [{ type: "text", text: `user ${IDS[8]}` }], timestamp: 1 },
		} as any,
		{
			type: "message",
			id: IDS[2],
			parentId: IDS[1],
			timestamp: "2026-07-23T00:00:00.000Z",
			message: {
				role: "assistant",
				provider: "unsafe provider",
				model: "unsafe model",
				content: [
					{ type: "text", text: "assistant visible" },
					{ type: "thinking", thinking: "THINKING-CANARY", signature: "SIGNATURE-CANARY" },
					{ type: "image", data: "IMAGE-CANARY", mimeType: "image/png" },
					{ type: "toolCall", id: "raw-tool-call-id", name: "unsafe tool", arguments: argumentsValue },
				],
			},
		} as any,
		{
			type: "message",
			id: IDS[3],
			parentId: IDS[2],
			timestamp: "2026-07-23T00:00:00.000Z",
			message: {
				role: "toolResult",
				toolCallId: "raw-tool-call-id",
				toolName: "unsafe tool",
				isError: false,
				content: [
					{ type: "text", text: "tool visible" },
					{ type: "image", data: "RESULT-IMAGE-CANARY", mimeType: "image/png" },
				],
				details: { secret: "TOOL-DETAILS-CANARY" },
			},
		} as any,
		custom(IDS[4], IDS[3], COMMAND_STATUS_TYPE, {
			commandName: "test:target",
			status: "completed",
			summary: "status visible",
		}),
		custom(IDS[5], IDS[4], "scramjet:log", {
			level: "warn",
			category: "unsafe category",
			message: "log visible",
			data: argumentsValue,
		}),
		{
			type: "compaction",
			id: IDS[6],
			parentId: IDS[5],
			timestamp: "2026-07-23T00:00:00.000Z",
			summary: "compaction visible",
			firstKeptEntryId: IDS[1],
			tokensBefore: 10,
			details: { secret: "COMPACTION-DETAILS-CANARY" },
		} as any,
		commandStart(IDS[7], IDS[6], "scramjet:troubleshoot"),
		message(IDS[8], IDS[7]),
	];
}

async function indexAll(h: ReturnType<typeof harness>, snapshotId: string) {
	const items: any[] = [];
	let request: any = { action: "index", snapshot_id: snapshotId };
	for (;;) {
		const page = await h.execute(request);
		expect(page.ok).toBe(true);
		items.push(...page.data.items);
		if (!page.data.next_cursor) return items;
		request = { action: "index", snapshot_id: snapshotId, cursor: page.data.next_cursor };
	}
}

async function readAll(h: ReturnType<typeof harness>, snapshotId: string, refs: string[]) {
	const items: any[] = [];
	let request: any = { action: "read", snapshot_id: snapshotId, evidence_refs: refs };
	for (;;) {
		const page = await h.execute(request);
		expect(page.ok).toBe(true);
		items.push(...page.data.items);
		if (!page.data.next_cursor) return items;
		request = { action: "read", snapshot_id: snapshotId, cursor: page.data.next_cursor };
	}
}

describe("exact evidence retrieval", () => {
	it("indexes content-free descriptors and reads exact class-specific evidence", async () => {
		const h = harness(evidenceEntries({ safe: true }));
		h.state.registry = new Map([
			[
				"test:target",
				{
					name: "test:target",
					filePath: "/must/not/appear",
					body: "current source body",
					description: "Current source",
					argumentHint: "[arg]",
				},
			],
		]);
		const { opened, selected } = await openAndSelect(h);
		expect(selected.data.current_source).toMatchObject({
			available: true,
			hash: expect.stringMatching(/^[0-9a-f]{64}$/),
		});
		expect(selected.data.authoring_guide).toMatchObject({
			available: true,
			hash: expect.stringMatching(/^[0-9a-f]{64}$/),
		});
		h.state.registry = new Map();

		const descriptors = await indexAll(h, opened.snapshot_id);
		for (const descriptor of descriptors) {
			expect(Object.keys(descriptor).sort()).toEqual([
				"class",
				"content_available",
				"evidence_ref",
				"fidelity",
				"subtype",
			]);
			expect(JSON.stringify(descriptor)).not.toContain("visible");
		}
		const pairs = descriptors.map((item) => [item.class, item.fidelity]);
		expect(pairs).toEqual(
			expect.arrayContaining([
				["transcript", "exact-partial"],
				["tool-call", "exact-partial"],
				["tool-result", "exact-partial"],
				["status", "summary"],
				["log", "diagnostic"],
				["compaction", "summary"],
				["source", "current-winning-candidate"],
				["guide", "normative"],
			]),
		);

		const evidence = await readAll(
			h,
			opened.snapshot_id,
			descriptors.slice(0, 12).map((item) => item.evidence_ref),
		);
		for (const item of evidence) {
			expect(Object.keys(item).sort()).toEqual([
				"chunk",
				"class",
				"content",
				"evidence_ref",
				"fidelity",
				"omissions",
			]);
			expect(Object.keys(item.chunk).sort()).toEqual(["complete", "sequence"]);
			expect(item.content.type).toBeTypeOf("string");
		}
		const source = evidence.find((item) => item.class === "source");
		expect(source.content).toMatchObject({ type: "current-source", body: "current source body" });
	});

	it("sanitizes JSON without invoking getters and excludes hidden or opaque payloads", async () => {
		let getterCalls = 0;
		const args = Object.create(null);
		args.safe = { z: 1, a: 2 };
		args.Authorization = "AUTH-CANARY";
		args.thinking = "THINKING-ARG-CANARY";
		Object.defineProperty(args, "accessor", {
			enumerable: true,
			get() {
				getterCalls++;
				throw new Error("GETTER-CANARY");
			},
		});
		const h = harness(evidenceEntries(args));
		const { opened } = await openAndSelect(h);
		const descriptors = await indexAll(h, opened.snapshot_id);
		const evidence = await readAll(
			h,
			opened.snapshot_id,
			descriptors.slice(0, 12).map((item) => item.evidence_ref),
		);
		expect(getterCalls).toBe(0);
		const toolCall = evidence.find((item) => item.class === "tool-call");
		expect(toolCall.content.arguments).toEqual({ safe: { a: 2, z: 1 } });
		expect(toolCall.content.call_ref).toMatch(/^cal-v1-/);
		const serialized = JSON.stringify(evidence);
		for (const canary of [
			"raw-tool-call-id",
			"AUTH-CANARY",
			"THINKING-CANARY",
			"THINKING-ARG-CANARY",
			"SIGNATURE-CANARY",
			"IMAGE-CANARY",
			"RESULT-IMAGE-CANARY",
			"TOOL-DETAILS-CANARY",
			"COMPACTION-DETAILS-CANARY",
			"GETTER-CANARY",
			"/must/not/appear",
		]) {
			expect(serialized).not.toContain(canary);
		}
	});

	it("authorizes only snapshot-issued evidence references", async () => {
		const first = harness(evidenceEntries({ safe: true }));
		const firstFlow = await openAndSelect(first);
		const firstDescriptors = await indexAll(first, firstFlow.opened.snapshot_id);
		expectError(
			await first.execute({
				action: "read",
				snapshot_id: firstFlow.opened.snapshot_id,
				evidence_refs: ["evd-v1-bbbbbbbbbbbbbbbbbbbbbbbbbb"],
			}),
			"UNKNOWN_REFERENCE",
		);

		const second = harness(evidenceEntries({ safe: true }));
		const secondFlow = await openAndSelect(second);
		expectError(
			await second.execute({
				action: "read",
				snapshot_id: secondFlow.opened.snapshot_id,
				evidence_refs: [firstDescriptors[0].evidence_ref],
			}),
			"UNKNOWN_REFERENCE",
		);
	});

	it("paginates inventories at the descriptor bound and binds cursors to their operation", async () => {
		const entries: SessionEntry[] = [commandStart(IDS[0], null, "test:many")];
		let parent = IDS[0];
		for (let index = 1; index <= 55; index++) {
			const id = index.toString(16).padStart(8, "0");
			entries.push({
				type: "message",
				id,
				parentId: parent,
				timestamp: "2026-07-23T00:00:00.000Z",
				message: { role: "user", content: `line ${index}`, timestamp: index },
			} as any);
			parent = id;
		}
		entries.push(commandStart(IDS[70], parent, "scramjet:troubleshoot"), message(IDS[71], IDS[70]));
		const h = harness(entries);
		const { opened } = await openAndSelect(h);
		const first = await h.execute({ action: "index", snapshot_id: opened.snapshot_id });
		expect(first.data.items).toHaveLength(50);
		expect(first.data.next_cursor).toMatch(/^cur-v1-/);
		expectError(
			await h.execute({ action: "read", snapshot_id: opened.snapshot_id, cursor: first.data.next_cursor }),
			"CURSOR_MISMATCH",
		);
		const second = await h.execute({
			action: "index",
			snapshot_id: opened.snapshot_id,
			cursor: first.data.next_cursor,
		});
		expect(second.data.items.length).toBeGreaterThan(0);
		expectError(
			await h.execute({ action: "index", snapshot_id: opened.snapshot_id, cursor: first.data.next_cursor }),
			"CURSOR_NOT_FOUND",
		);
		expect(new Set([...first.data.items, ...second.data.items].map((item: any) => item.evidence_ref)).size).toBe(
			first.data.items.length + second.data.items.length,
		);
	});

	it("returns immutable fixed gaps on select, index, and read", async () => {
		const h = harness(evidenceEntries({ safe: true }));
		const { opened, selected } = await openAndSelect(h);
		expect(selected.gaps).toEqual(
			expect.arrayContaining([
				EVIDENCE_GAPS.SOURCE_UNAVAILABLE,
				EVIDENCE_GAPS.HIDDEN_CONTENT_EXCLUDED,
				EVIDENCE_GAPS.IMAGE_CONTENT_EXCLUDED,
				EVIDENCE_GAPS.DETAILS_EXCLUDED,
				EVIDENCE_GAPS.IDENTIFIERS_PSEUDONYMIZED,
			]),
		);
		const indexed = await h.execute({ action: "index", snapshot_id: opened.snapshot_id });
		expect(indexed.gaps).toEqual(selected.gaps);
		const read = await h.execute({
			action: "read",
			snapshot_id: opened.snapshot_id,
			evidence_refs: [indexed.data.items[0].evidence_ref],
		});
		expect(read.gaps).toEqual(selected.gaps);
		expect(Object.isFrozen(selected.gaps)).toBe(true);
		for (const gap of selected.gaps) {
			expect(Object.keys(gap).sort()).toEqual(["code", "message"]);
			expect(Object.isFrozen(gap)).toBe(true);
		}
	});

	it("normalizes dangerous keys, rejects prototype keys, scrubs raw IDs in keys, and emits null-prototype objects", async () => {
		const args = Object.create(null);
		for (const key of [
			"apiKey",
			"access_token",
			"ANTHROPIC_API_KEY",
			"clientSecret",
			"privateKey",
			"connectionString",
			"proxyAuthorization",
			"sessionCookie",
			"oauthToken",
			"databasePassword",
			"sshPassphrase",
			"signingSecret",
			"cloudCredential",
			"__proto__",
			"prototype",
			"constructor",
		]) {
			Object.defineProperty(args, key, { value: `${key}-CANARY`, enumerable: true });
		}
		args[`raw-${IDS[8]}`] = "scrubbed key value";
		args.safe = true;
		const h = harness(evidenceEntries(args));
		const { opened } = await openAndSelect(h);
		const descriptors = await indexAll(h, opened.snapshot_id);
		const tool = (
			await readAll(h, opened.snapshot_id, [descriptors.find((item) => item.class === "tool-call").evidence_ref])
		)[0];
		expect(tool.content.arguments).toEqual({ "raw-scramjet-id": "scrubbed key value", safe: true });
		expect(Object.getPrototypeOf(tool.content.arguments)).toBeNull();
		const serialized = JSON.stringify(tool);
		for (const key of [
			"apiKey",
			"access_token",
			"ANTHROPIC_API_KEY",
			"clientSecret",
			"privateKey",
			"connectionString",
			"proxyAuthorization",
			"sessionCookie",
			"oauthToken",
			"databasePassword",
			"sshPassphrase",
			"signingSecret",
			"cloudCredential",
		]) {
			expect(serialized).not.toContain(`${key}-CANARY`);
		}
		expect(serialized).not.toContain(IDS[8]);
	});

	it("uses separate identifier domains while preserving ordinary tool names", async () => {
		const entries = evidenceEntries({ safe: true });
		const assistant = entries[2] as any;
		assistant.message.content.push(
			{ type: "toolCall", id: "read-call", name: "read", arguments: {} },
			{ type: "toolCall", id: "bash-call", name: "bash", arguments: {} },
		);
		entries.splice(6, 0, {
			type: "custom_message",
			id: "00000009",
			parentId: IDS[5],
			timestamp: "2026-07-23T00:00:00.000Z",
			customType: "unsafe value",
			content: "custom",
			display: true,
		} as any);
		(entries[7] as any).parentId = "00000009";
		const h = harness(entries);
		const { opened } = await openAndSelect(h);
		const descriptors = await indexAll(h, opened.snapshot_id);
		const evidence = await readAll(
			h,
			opened.snapshot_id,
			descriptors
				.filter((item) => ["tool-call", "log", "transcript"].includes(item.class))
				.slice(0, 12)
				.map((item) => item.evidence_ref),
		);
		const tools = evidence.filter((item) => item.class === "tool-call").map((item) => item.content.tool);
		expect(tools).toEqual(expect.arrayContaining(["read", "bash", expect.stringMatching(/^tol-v1-/)]));
		expect(evidence.find((item) => item.content.type === "custom-transcript")?.content.subtype).toMatch(/^sub-v1-/);
		expect(evidence.find((item) => item.class === "log")?.content.category).toMatch(/^cat-v1-/);
	});

	it("bounds source metadata arrays and reports truncation", async () => {
		const h = harness(evidenceEntries({ safe: true }));
		h.state.registry = new Map([
			[
				"test:target",
				{
					name: "test:target",
					filePath: "/not-exposed",
					body: "source",
					allowedTools: Array.from({ length: 40 }, (_, index) => `tool-${index}`),
				},
			],
		]);
		const { opened, selected } = await openAndSelect(h);
		expect(selected.gaps).toContain(EVIDENCE_GAPS.CONTENT_TRUNCATED);
		const descriptors = await indexAll(h, opened.snapshot_id);
		const sourceRef = descriptors.find((item) => item.class === "source").evidence_ref;
		const source = (await readAll(h, opened.snapshot_id, [sourceRef]))[0];
		expect(source.content.allowed_tools).toHaveLength(32);
	});

	it("rejects oversized generated evidence before selecting and does not bypass logical line limits", async () => {
		const many: SessionEntry[] = [commandStart("00001000", null, "test:large")];
		let parent = "00001000";
		for (let index = 1; index <= 600; index++) {
			const id = (0x1000 + index).toString(16).padStart(8, "0");
			many.push({
				type: "message",
				id,
				parentId: parent,
				timestamp: "2026-07-23T00:00:00.000Z",
				message: { role: "user", content: "x".repeat(2_000), timestamp: index },
			} as any);
			parent = id;
		}
		many.push(commandStart("00002000", parent, "scramjet:troubleshoot"), message("00002001", "00002000"));
		const large = harness(many);
		const opened = await large.execute({ action: "open" });
		const request = {
			action: "select",
			snapshot_id: opened.snapshot_id,
			target_ref: opened.data.proposed_target_ref,
		};
		expectError(await large.execute(request), "INVOCATION_LIMIT");
		expectError(await large.execute(request), "INVOCATION_LIMIT");

		const linedEntries = [
			commandStart(IDS[0], null, "test:lines"),
			{
				type: "message",
				id: IDS[1],
				parentId: IDS[0],
				timestamp: "2026-07-23T00:00:00.000Z",
				message: { role: "user", content: Array.from({ length: 201 }, () => "line").join("\n") },
			} as any,
			commandStart(IDS[2], IDS[1], "scramjet:troubleshoot"),
			message(IDS[3], IDS[2]),
		];
		const lined = harness(linedEntries);
		const linedOpen = await lined.execute({ action: "open" });
		expectError(
			await lined.execute({
				action: "select",
				snapshot_id: linedOpen.snapshot_id,
				target_ref: linedOpen.data.proposed_target_ref,
			}),
			"INVOCATION_LIMIT",
		);
	});

	it("reports unsupported content and unknown enum mappings without exposing their values", async () => {
		const entries = evidenceEntries({ safe: true });
		(entries[2] as any).message.content.push({ type: "audio", data: "UNSUPPORTED-CANARY" });
		(entries[4] as any).data.status = "HOSTILE-STATUS";
		(entries[5] as any).data.level = "HOSTILE-LEVEL";
		const h = harness(entries);
		const { selected } = await openAndSelect(h);
		expect(selected.gaps).toEqual(
			expect.arrayContaining([EVIDENCE_GAPS.UNSUPPORTED_CONTENT_EXCLUDED, EVIDENCE_GAPS.UNKNOWN_ENUM_MAPPED]),
		);
		expect(JSON.stringify(selected)).not.toContain("HOSTILE");
		expect(JSON.stringify(selected)).not.toContain("UNSUPPORTED-CANARY");
	});

	it("cascades cursors when their snapshot is evicted", async () => {
		const entries: SessionEntry[] = [commandStart(IDS[0], null, "test:many")];
		let parent = IDS[0];
		for (let index = 1; index <= 55; index++) {
			const id = index.toString(16).padStart(8, "0");
			entries.push({
				type: "message",
				id,
				parentId: parent,
				timestamp: "2026-07-23T00:00:00.000Z",
				message: { role: "user", content: `line ${index}` },
			} as any);
			parent = id;
		}
		entries.push(commandStart(IDS[70], parent, "scramjet:troubleshoot"), message(IDS[71], IDS[70]));
		const h = harness(entries);
		const first = await openAndSelect(h);
		const firstPage = await h.execute({ action: "index", snapshot_id: first.opened.snapshot_id });
		const evictedCursor = firstPage.data.next_cursor;
		expect(evictedCursor).toMatch(/^cur-v1-/);

		let newest: any;
		for (let index = 0; index < 4; index++) newest = await openAndSelect(h);
		expectError(
			await h.execute({ action: "index", snapshot_id: newest.opened.snapshot_id, cursor: evictedCursor }),
			"CURSOR_NOT_FOUND",
		);
	});

	it("bounds array, object, proxy, and message-block inspection before accumulation", async () => {
		let arrayGetterCalls = 0;
		let blockGetterCalls = 0;
		let proxyOwnKeysCalls = 0;
		const longArray = Array.from({ length: MAX_SAFE_JSON_ITEMS }, (_, index) => index);
		Object.defineProperty(longArray, String(MAX_SAFE_JSON_ITEMS), {
			enumerable: true,
			get() {
				arrayGetterCalls++;
				return "ARRAY-CANARY";
			},
		});
		const oversized = Object.fromEntries(
			Array.from({ length: MAX_SAFE_JSON_ITEMS + 1 }, (_, index) => [`key${index}`, index]),
		);
		const hostileProxy = new Proxy(
			{},
			{
				ownKeys() {
					proxyOwnKeysCalls++;
					throw new Error("PROXY-OWN-KEYS-CANARY");
				},
			},
		);
		const entries = evidenceEntries({ longArray, oversized, hostileProxy });
		const content = Array.from({ length: MAX_MESSAGE_CONTENT_BLOCKS }, (_, index) =>
			index === 0
				? { type: "toolCall", id: "bounded-call", name: "read", arguments: { longArray, oversized, hostileProxy } }
				: { type: "text", text: `visible ${index}` },
		);
		Object.defineProperty(content, String(MAX_MESSAGE_CONTENT_BLOCKS), {
			enumerable: true,
			get() {
				blockGetterCalls++;
				return { type: "toolCall", id: "BLOCK-CANARY", name: "read", arguments: {} };
			},
		});
		(entries[2] as any).message.content = content;
		const h = harness(entries);
		const { opened, selected } = await openAndSelect(h);
		expect(selected.gaps).toContain(EVIDENCE_GAPS.CONTENT_TRUNCATED);
		const descriptors = await indexAll(h, opened.snapshot_id);
		expect(descriptors.length).toBeLessThanOrEqual(1_600);
		const toolRef = descriptors.find((item) => item.class === "tool-call")?.evidence_ref;
		expect(toolRef).toBeDefined();
		const tool = (await readAll(h, opened.snapshot_id, [toolRef]))[0];
		expect(tool.content.arguments.longArray).toHaveLength(MAX_SAFE_JSON_ITEMS + 1);
		expect(tool.content.arguments.oversized).toEqual({ $scramjet: "truncated", reason: "items" });
		expect(tool.content.arguments.hostileProxy).toEqual({ $scramjet: "truncated", reason: "unsupported-value" });
		expect(arrayGetterCalls).toBe(0);
		expect(blockGetterCalls).toBe(0);
		expect(proxyOwnKeysCalls).toBe(0);
		expect(JSON.stringify(tool)).not.toContain("CANARY");
	});

	it("returns the frozen first selection after live model and branch content changes", async () => {
		const entries = evidenceEntries({ safe: true });
		const h = harness(entries);
		const { opened, target, selected } = await openAndSelect(h);
		const initialIndex = await indexAll(h, opened.snapshot_id);
		h.setModel("changed-provider", "changed-model");
		(entries[2] as any).message.provider = "appended-provider";
		(entries[2] as any).message.model = "appended-model";
		(entries[2] as any).message.content.push({
			type: "toolCall",
			id: "late-call",
			name: "late-tool",
			arguments: { late: true },
		});
		const appended = {
			type: "message",
			id: IDS[9],
			parentId: IDS[8],
			timestamp: "2026-07-23T00:00:01.000Z",
			message: { role: "assistant", provider: "live-provider", model: "live-model", content: [] },
		} as any;
		h.setEntries([...entries, appended]);
		const repeated = await h.execute({
			action: "select",
			snapshot_id: opened.snapshot_id,
			target_ref: target.target_ref,
		});
		expect(repeated).toEqual(selected);
		expect(Object.isFrozen(repeated.data)).toBe(true);
		expect(Object.isFrozen(repeated.data.execution_models)).toBe(true);
		expect(Object.isFrozen(repeated.data.troubleshooting_model)).toBe(true);
		expect(await indexAll(h, opened.snapshot_id)).toEqual(initialIndex);
		expect(JSON.stringify(repeated)).not.toContain("changed-");
		expect(JSON.stringify(repeated)).not.toContain("appended-");
		expect(JSON.stringify(repeated)).not.toContain("live-");
	});

	it("pseudonymizes malformed non-string identifiers in their own domains", async () => {
		const entries = evidenceEntries({ safe: true });
		(entries[2] as any).message.provider = { malformed: true };
		(entries[2] as any).message.model = 42;
		const call = (entries[2] as any).message.content.find((block: any) => block.type === "toolCall");
		call.id = { malformed: true };
		call.name = 17;
		(entries[3] as any).message.toolCallId = null;
		(entries[3] as any).message.toolName = { malformed: true };
		const h = harness(entries);
		const { opened, selected } = await openAndSelect(h);
		expect(selected.gaps).toContain(EVIDENCE_GAPS.IDENTIFIERS_PSEUDONYMIZED);
		expect(selected.gaps).not.toContain(EVIDENCE_GAPS.UNKNOWN_ENUM_MAPPED);
		expect(selected.data.execution_models).toContainEqual({
			provider: expect.stringMatching(/^prv-v1-/),
			model: expect.stringMatching(/^mdl-v1-/),
		});
		const descriptors = await indexAll(h, opened.snapshot_id);
		const evidence = await readAll(
			h,
			opened.snapshot_id,
			descriptors
				.filter((item) => item.class === "tool-call" || item.class === "tool-result")
				.map((item) => item.evidence_ref),
		);
		const toolCall = evidence.find((item) => item.class === "tool-call");
		const toolResult = evidence.find((item) => item.class === "tool-result");
		expect(toolCall.content.tool).toMatch(/^tol-v1-/);
		expect(toolResult.content.tool).toMatch(/^tol-v1-/);
		expect(toolCall.content.call_ref).toMatch(/^cal-v1-/);
		expect(toolResult.content.call_ref).toMatch(/^cal-v1-/);
		expect(toolCall.content.call_ref).not.toBe(toolResult.content.call_ref);
		expect(JSON.stringify({ selected, evidence })).not.toContain('"unknown"');
	});
});
