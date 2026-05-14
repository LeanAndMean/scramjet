import { describe, expect, it, vi } from "vitest";

// Replace Pi's tool factories with stubs that echo back the cwd they were
// constructed with, plus the arguments their execute was invoked with. Lets
// us assert the load-bearing invariant that each alias rebuilds its factory
// per call against ctx.cwd, without dragging the real Pi runtime into a
// unit test.
vi.mock("@earendil-works/pi-coding-agent", () => {
	const makeFactory = (piName: string) => (cwd: string) => ({
		name: piName,
		label: piName,
		description: `${piName} description`,
		parameters: { type: "object", properties: {} },
		async execute(toolCallId: string, params: unknown, signal: unknown, onUpdate: unknown, ctx: unknown) {
			return { factoryCwd: cwd, toolCallId, params, signal, onUpdate, ctx };
		},
	});
	return {
		createReadToolDefinition: makeFactory("read"),
		createBashToolDefinition: makeFactory("bash"),
		createEditToolDefinition: makeFactory("edit"),
		createWriteToolDefinition: makeFactory("write"),
		createGrepToolDefinition: makeFactory("grep"),
		createFindToolDefinition: makeFactory("find"),
		createLsToolDefinition: makeFactory("ls"),
	};
});

import { CLAUDE_CODE_TOOL_NAMES, registerToolAliases } from "../src/tool-aliases/index.ts";

function recordingPi() {
	const registered: any[] = [];
	const pi: any = {
		registerTool(tool: any) {
			registered.push(tool);
		},
	};
	return { pi, registered };
}

describe("CLAUDE_CODE_TOOL_NAMES", () => {
	it("exposes the seven Claude Code tool names in stable order", () => {
		expect(CLAUDE_CODE_TOOL_NAMES).toEqual(["Read", "Bash", "Edit", "Write", "Grep", "Glob", "LS"]);
	});
});

describe("registerToolAliases", () => {
	it("registers exactly seven aliases", () => {
		const { pi, registered } = recordingPi();
		registerToolAliases(pi);
		expect(registered).toHaveLength(7);
	});

	it("overrides name and label to the PascalCase Claude Code name", () => {
		const { pi, registered } = recordingPi();
		registerToolAliases(pi);
		expect(registered.map((t) => t.name)).toEqual([...CLAUDE_CODE_TOOL_NAMES]);
		expect(registered.map((t) => t.label)).toEqual([...CLAUDE_CODE_TOOL_NAMES]);
	});

	it("rebuilds the factory with ctx.cwd per call (not process.cwd at registration time)", async () => {
		const { pi, registered } = recordingPi();
		registerToolAliases(pi);
		const readAlias = registered.find((t) => t.name === "Read");
		const result = await readAlias.execute("call-1", { path: "a" }, undefined, undefined, {
			cwd: "/per-call/cwd",
		});
		expect(result.factoryCwd).toBe("/per-call/cwd");
		expect(result.factoryCwd).not.toBe(process.cwd());
	});

	it("forwards toolCallId, params, signal, onUpdate, ctx unchanged to the underlying factory", async () => {
		const { pi, registered } = recordingPi();
		registerToolAliases(pi);
		const bashAlias = registered.find((t) => t.name === "Bash");
		const signal = new AbortController().signal;
		const onUpdate = () => {};
		const params = { command: "echo hi" };
		const ctx = { cwd: "/tmp/x" };
		const result = await bashAlias.execute("call-2", params, signal, onUpdate, ctx);
		expect(result.toolCallId).toBe("call-2");
		expect(result.params).toBe(params);
		expect(result.signal).toBe(signal);
		expect(result.onUpdate).toBe(onUpdate);
		expect(result.ctx).toBe(ctx);
	});
});
