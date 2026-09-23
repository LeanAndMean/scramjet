import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createProductionInteractiveHarness } from "../../coding-agent/tests/helpers/interactive-harness.js";
import { sliceByColumn } from "../../tui/src/utils.js";
import { registerSubagentTool } from "../src/subagent/index.js";
import { noOpTerminalIndicators } from "./helpers.js";

vi.mock("../../coding-agent/src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

let harness: Awaited<ReturnType<typeof createProductionInteractiveHarness>> | undefined;

afterEach(async () => {
	await harness?.dispose();
	harness = undefined;
	vi.useRealTimers();
});

async function runningBatch(viewport = false) {
	harness = await createProductionInteractiveHarness(
		48,
		12,
		(pi) => {
			registerSubagentTool(pi, noOpTerminalIndicators());
		},
		viewport,
	);
	const tasks = Array.from({ length: 8 }, (_, index) => ({ agent: `child-${index + 1}`, task: "Synthetic task" }));
	await harness.emit({ type: "agent_start" });
	await harness.emit({ type: "tool_execution_start", toolCallId: "batch", toolName: "subagent", args: { tasks } });
	const partialResult = {
		content: [{ type: "text", text: "Synthetic partial batch" }],
		details: {
			mode: "parallel",
			agentScope: "user",
			projectAgentsDir: null,
			results: tasks.map((task, index) => ({
				...task,
				agentSource: "user",
				exitCode: index < 4 ? 0 : -1,
				messages: [
					{
						role: "assistant",
						content: [
							{
								type: "text",
								text: `CARD-${index + 1} ${"uniquely labelled wrapped synthetic output ".repeat(4)}`,
							},
						],
					},
				],
				stderr: "",
				usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, contextTokens: 0, turns: 0 },
			})),
		},
	};
	await harness.emit({
		type: "tool_execution_update",
		toolCallId: "batch",
		toolName: "subagent",
		args: { tasks },
		partialResult,
	});
	await harness.frame();
	return { ...harness, partialResult, tasks };
}

describe("production running subagent presentation", () => {
	beforeEach(async () => {
		await runningBatch();
	});

	it("characterizes live rows omitted before transport, not producer truncation", async () => {
		const h = harness!;
		const logicalRows = h.internals.chatContainer.render(48);
		expect(logicalRows.length).toBeGreaterThan(3 * h.terminal.rows);
		for (let index = 1; index <= 8; index++) expect(logicalRows.join("\n")).toContain(`CARD-${index}`);
		expect(h.terminal.bufferLines().join("\n")).not.toContain("CARD-1");
		h.terminal.scrollLines(-1000);
		expect(h.terminal.visibleLines().join("\n")).not.toContain("CARD-1");
	});

	it.fails("documents inaccessible running cards under the original tail-only route (#551)", async () => {
		const h = harness!;
		const seen = new Set(h.terminal.visibleLines());
		for (let index = 0; index < 100; index++) {
			h.terminal.sendInput("\x1b[<64;10;3M");
			for (const row of await h.frame()) seen.add(row);
		}
		expect([...seen].join("\n")).toContain("CARD-1");
	});
});

async function browseAll(h: NonNullable<typeof harness>): Promise<string> {
	const seen = new Set<string>();
	h.internals.ui.scrollViewportTo(Number.MAX_SAFE_INTEGER);
	for (let index = 0; index < 150; index++) {
		for (const row of await h.frame()) seen.add(row);
		if (h.internals.ui.getViewportState()!.offset === 0) break;
		h.terminal.sendInput("\x1b[<64;10;3M");
	}
	return [...seen].join("\n");
}

it("runs eight children on four workers and browses interleaved live and final cards without duplication", async () => {
	harness = await createProductionInteractiveHarness(
		60,
		12,
		(pi) => registerSubagentTool(pi, noOpTerminalIndicators()),
		true,
	);
	const h = harness;
	const cwd = h.session.sessionManager.getCwd();
	mkdirSync(join(cwd, ".scramjet", "agents"), { recursive: true });
	writeFileSync(join(cwd, ".scramjet", "agents", "child.md"), "---\nname: child\ndescription: Synthetic child\n---\n");
	const script = join(cwd, "child.cjs");
	writeFileSync(
		script,
		`
		const fs = require('node:fs');
		let input = '';
		process.stdin.on('data', chunk => input += chunk);
		process.stdin.on('end', () => {
			const id = Number(input.match(/TASK-(\\d+)/)[1]);
			fs.writeFileSync('started-' + id, '');
			const text = 'OUTPUT-' + id + '\\n' + Array.from({length: 16}, (_, n) => 'detail-' + id + '-' + n).join('\\n');
			console.log(JSON.stringify({type: 'message_end', message: {role: 'assistant', content: [{type: 'text', text}]}}));
			const timer = setInterval(() => { if (fs.existsSync('release-' + id)) { clearInterval(timer); process.exit(0); } }, 5);
		});
	`,
	);
	const argv = process.argv[1];
	process.argv[1] = script;
	const controller = new AbortController();
	const tasks = Array.from({ length: 8 }, (_, i) => ({ agent: "child", task: `TASK-${i + 1}` }));
	const args = { tasks, agentScope: "project" as const, confirmProjectAgents: false };
	const tool = h.session.extensionRunner
		.getAllRegisteredTools()
		.find((tool) => tool.definition.name === "subagent")!.definition;
	let updates = Promise.resolve();
	let done = false;
	await h.emit({ type: "agent_start" });
	await h.emit({ type: "tool_execution_start", toolCallId: "batch", toolName: "subagent", args });
	const component = h.internals.chatContainer.children[0];
	const execution = tool
		.execute(
			"batch",
			args,
			controller.signal,
			(partialResult) => {
				const snapshot = structuredClone(partialResult);
				updates = updates.then(() =>
					h.emit({
						type: "tool_execution_update",
						toolCallId: "batch",
						toolName: "subagent",
						args,
						partialResult: snapshot,
					}),
				);
			},
			h.session.extensionRunner.createContext(),
		)
		.then((result) => {
			done = true;
			return result;
		});
	try {
		await vi.waitFor(() => expect([1, 2, 3, 4].every((i) => existsSync(join(cwd, `started-${i}`)))).toBe(true));
		expect(existsSync(join(cwd, "started-5"))).toBe(false);
		await vi.waitFor(async () => {
			await updates;
			for (let i = 1; i <= 4; i++) expect(h.internals.chatContainer.render(59).join("\n")).toContain(`OUTPUT-${i}`);
		});
		const firstWave = await browseAll(h);
		for (let i = 1; i <= 4; i++) expect(firstWave).toContain(`OUTPUT-${i}`);
		for (let i = 5; i <= 8; i++) expect(existsSync(join(cwd, `started-${i}`))).toBe(false);
		expect(firstWave).not.toContain("detail-1-15");
		h.internals.setToolsExpanded(true);
		expect(await browseAll(h)).toContain("detail-1-15");
		expect(done).toBe(false);
		for (const [finished, next] of [
			[4, 5],
			[3, 6],
			[2, 7],
			[1, 8],
		]) {
			writeFileSync(join(cwd, `release-${finished}`), "");
			await vi.waitFor(() => expect(existsSync(join(cwd, `started-${next}`))).toBe(true));
		}
		await vi.waitFor(async () => {
			await updates;
			for (let i = 1; i <= 8; i++) expect(h.internals.chatContainer.render(59).join("\n")).toContain(`OUTPUT-${i}`);
		});
		const secondWave = await browseAll(h);
		for (let i = 1; i <= 8; i++) expect(secondWave).toContain(`OUTPUT-${i}`);
		expect(done).toBe(false);
		for (const id of [8, 7, 6, 5]) writeFileSync(join(cwd, `release-${id}`), "");
		const result = await execution;
		await updates;
		await h.emit({ type: "tool_execution_end", toolCallId: "batch", toolName: "subagent", result, isError: false });
		await h.emit({ type: "agent_end", messages: [] });
		expect(h.internals.committedChatContainer.children).toContain(component);
		expect(h.internals.chatContainer.children).not.toContain(component);
		for (const expanded of [false, true, false]) {
			h.internals.setToolsExpanded(expanded);
			const visible = await browseAll(h);
			const logical = h.internals.committedChatContainer.render(59).join("\n");
			for (let i = 1; i <= 8; i++) {
				expect(visible).toContain(`OUTPUT-${i}`);
				expect(logical.match(new RegExp(`OUTPUT-${i}`, "g"))).toHaveLength(1);
				if (expanded) expect(visible).toContain(`detail-${i}-15`);
			}
		}
	} finally {
		controller.abort();
		try {
			await execution.catch(() => {});
			await updates;
		} finally {
			process.argv[1] = argv;
		}
	}
}, 20000);

it("keeps card separators, reading anchors and held selection coherent during batch updates", async () => {
	const h = await runningBatch(true);
	for (const [index, result] of h.partialResult.details.results.entries()) {
		result.messages[0].content[0].text = `CARD-${index + 1}\ndetail-${index + 1}`;
	}
	const update = () =>
		h.emit({
			type: "tool_execution_update",
			toolCallId: "batch",
			toolName: "subagent",
			args: { tasks: h.tasks },
			partialResult: h.partialResult,
		});
	const rows = async () => (await h.frame()).map((row) => sliceByColumn(row, 0, 47, true).trimEnd());
	await update();
	await h.frame();
	const cardRow = () => h.internals.ui.render(47).findIndex((row) => row.includes("CARD-3"));
	expect(cardRow()).toBeGreaterThan(0);
	h.internals.ui.scrollViewportTo(cardRow());
	const expected = [
		" CARD-3",
		" detail-3",
		"",
		" ─── child-4 ✓ [Effort:off]",
		" CARD-4",
		" detail-4",
		"",
		" ─── child-5 ⏳ [Effort:off]",
		" CARD-5",
		" detail-5",
		"",
		" ─── child-6 ⏳ [Effort:off]",
	];
	expect(await rows()).toEqual(expected);
	h.partialResult.details.results[0].messages[0].content[0].text = "INSERTED\nCARD-1\ndetail-1";
	await update();
	expect(await rows()).toEqual(expected);
	h.terminal.sendInput("\x1b[<0;2;1M");
	h.terminal.sendInput("\x1b[<32;8;1M");
	h.terminal.sendInput("\x1b[<0;8;1m");
	h.partialResult.details.results[2].messages[0].content[0].text = "CARD-3\nREPLACED";
	await update();
	expect(await rows()).toEqual(expected);
	for (let col = 1; col < 7; col++) expect(h.terminal.cell(0, col).inverse).toBe(true);
	expect(h.terminal.cell(0, 47).inverse).toBe(false);
	h.terminal.sendInput("\x1b");
	await h.frame();
	h.internals.ui.scrollViewportTo(cardRow());
	expect(await rows()).toEqual([expected[0], " REPLACED", ...expected.slice(2)]);
	expect(h.terminal.cell(0, 1).inverse).toBe(false);
	expect(h.extensionUI.getEditorText()).toBe("");
	expect(h.internals.chatContainer.children).toHaveLength(1);
});

it("keeps every presented card reachable by wheel before outer completion through production configuration", async () => {
	const h = await runningBatch(true);
	const seen = new Set(await h.frame());
	for (let index = 0; index < 100; index++) {
		h.terminal.sendInput("\x1b[<64;10;3M");
		for (const row of await h.frame()) seen.add(row);
	}
	for (let index = 1; index <= 8; index++) expect([...seen].join("\n")).toContain(`CARD-${index}`);
	expect(h.extensionUI.getEditorText()).toBe("");
});
