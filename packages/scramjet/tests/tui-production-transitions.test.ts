import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createProductionInteractiveHarness } from "../../coding-agent/tests/helpers/interactive-harness.js";
import { registerSubagentTool } from "../src/subagent/index.js";
import { noOpTerminalIndicators } from "./helpers.js";

let harness: Awaited<ReturnType<typeof createProductionInteractiveHarness>> | undefined;

afterEach(async () => {
	await harness?.dispose();
	harness = undefined;
	vi.useRealTimers();
});

async function runningBatch() {
	harness = await createProductionInteractiveHarness(48, 12, (pi) => {
		registerSubagentTool(pi, noOpTerminalIndicators());
	});
	const tasks = Array.from({ length: 8 }, (_, index) => ({ agent: `child-${index + 1}`, task: "Synthetic task" }));
	await harness.emit({ type: "agent_start" });
	await harness.emit({ type: "tool_execution_start", toolCallId: "batch", toolName: "subagent", args: { tasks } });
	await harness.emit({
		type: "tool_execution_update",
		toolCallId: "batch",
		toolName: "subagent",
		args: { tasks },
		partialResult: {
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
		},
	});
	await harness.frame();
	return harness;
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

	it.fails("keeps the first running-batch card reachable by wheel before outer completion (#551)", async () => {
		const h = harness!;
		const seen = new Set(h.terminal.visibleLines());
		for (let index = 0; index < 100; index++) {
			h.terminal.sendInput("\x1b[<64;10;3M");
			for (const row of await h.frame()) seen.add(row);
		}
		expect([...seen].join("\n")).toContain("CARD-1");
	});
});
