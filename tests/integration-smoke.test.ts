import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerAutoContinue } from "../auto-continue.ts";
import { registerCommandStatusTool } from "../command-status.ts";
import { parseCommandFile } from "../commands/loader.ts";
import { registerDelegateTool } from "../delegate.ts";
import { registerHistory } from "../history.ts";
import scramjet from "../index.ts";
import { registerScramjetCommand } from "../scramjet-command.ts";
import { registerToolCallAdvisor } from "../tool-scope-advisory.ts";
import type { CommandDef, NextStepPolicy, ScramjetState } from "../types.ts";
import { freshState, recordingPi } from "./helpers.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const MACH12_COMMANDS_DIR = resolve(HERE, "..", "mach12", "commands");
const SET_NAME = "mach12";

// Subroutines (delegate-only command files with no `next:` block). The
// integration smoke confirms each one is loadable from disk and that the
// delegate tool returns the substituted body when invoked against the real
// file. The list lives here rather than being discovered dynamically so a
// stray accidentally-delegate-only top-level command would fail the
// `mach12-wiring.test.ts` count assertion before reaching this test.
const SUBROUTINES = [
	"push",
	"find-contribution-guidelines",
	"gh-issue-read",
	"gh-pr-read",
	"gh-sub-issues",
	"gh-assign",
	"gh-comment",
];

function loadCommand(basename: string): CommandDef {
	const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
	const content = readFileSync(filePath, "utf-8");
	const result = parseCommandFile(filePath, content, SET_NAME);
	if (!result.ok) throw new Error(`failed to parse ${filePath}: ${result.error}`);
	return result.def;
}

function seedRegistry(defs: CommandDef[]): ScramjetState {
	return freshState({ registry: new Map(defs.map((d) => [d.name, d])) });
}

describe("integration smoke — delegate against real mach12 subroutines", () => {
	it.each(SUBROUTINES)(
		"delegates to mach12:%s, returns substituted body with $ARGUMENTS expanded",
		async (basename) => {
			const def = loadCommand(basename);
			const state = seedRegistry([def]);
			const { pi, tools } = recordingPi();
			registerDelegateTool(pi, state);
			const tool = tools[0];

			const probe = `<<SCRAMJET_PROBE_${basename}>>`;
			const result = await tool.execute(
				`call-${basename}`,
				{ command: `${SET_NAME}:${basename}`, args: probe },
				undefined,
				undefined,
				{ cwd: "/" },
			);

			expect(result.details.error).toBeUndefined();
			expect(result.details.command).toBe(`${SET_NAME}:${basename}`);
			expect(result.content).toHaveLength(1);
			expect(result.content[0].type).toBe("text");
			const body = result.content[0].text as string;
			// The body returned is the substituted command body; the file source
			// itself must contain $ARGUMENTS to verify substitution worked here.
			// All subroutines that take args reference $ARGUMENTS in their prose;
			// find-contribution-guidelines takes no args so the probe just trails
			// off into the body unchanged.
			if (def.body.includes("$ARGUMENTS")) {
				expect(body).toContain(probe);
				expect(body).not.toContain("$ARGUMENTS");
			} else {
				expect(body).toEqual(def.body);
			}
			expect(state.delegateStack).toHaveLength(1);
			expect(state.delegateStack[0].commandName).toBe(`${SET_NAME}:${basename}`);
		},
	);

	it("loads every subroutine from disk without parse errors", () => {
		// Defense in depth: ensures every file we expect to be present
		// actually parses, even ones whose body has no $ARGUMENTS to verify
		// substitution. Catches malformed frontmatter regressions.
		const onDisk = readdirSync(MACH12_COMMANDS_DIR)
			.filter((f) => f.endsWith(".md"))
			.map((f) => f.replace(/\.md$/, "").replace(`${SET_NAME}:`, ""));
		for (const basename of SUBROUTINES) {
			expect(onDisk).toContain(basename);
			expect(() => loadCommand(basename)).not.toThrow();
		}
	});
});

describe("integration smoke — advisory warning against real subroutine scope", () => {
	let warnSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	afterEach(() => {
		warnSpy.mockRestore();
	});

	it("fires the advisory warning for a tool outside the delegated frame's allowed-tools", async () => {
		// gh-issue-read declares allowed-tools: [bash] -- a tight scope that
		// makes "Edit" obviously out-of-scope. Using a real subroutine confirms
		// the allowed-tools array we author propagates from disk into the
		// active frame's effectiveAllowedTools.
		const def = loadCommand("gh-issue-read");
		expect(def.allowedTools).toBeDefined();
		expect(def.allowedTools).toEqual(["bash"]);

		const state = seedRegistry([def]);
		const { pi, tools, handlers } = recordingPi();
		registerDelegateTool(pi, state);
		registerToolCallAdvisor(pi, state);
		const delegateTool = tools[0];

		await delegateTool.execute(
			"call-advisory",
			{ command: "mach12:gh-issue-read", args: "55" },
			undefined,
			undefined,
			{ cwd: "/" },
		);
		expect(state.delegateStack).toHaveLength(1);
		expect(state.delegateStack[0].effectiveAllowedTools).toEqual(["bash"]);

		const toolCallHandler = handlers.get("tool_call")![0] as any;
		await toolCallHandler({ type: "tool_call", toolCallId: "x", toolName: "Edit", input: {} });

		expect(warnSpy).toHaveBeenCalledTimes(1);
		const message = String(warnSpy.mock.calls[0][0]);
		expect(message).toContain("[scramjet]");
		expect(message).toContain("advisory");
		expect(message).toContain("Edit");
		expect(message).toContain("mach12:gh-issue-read");
		expect(message).toContain("depth=1");
		expect(message).toContain("bash");
	});

	it("does not warn when the called tool is in the delegated frame's allowed-tools", async () => {
		const def = loadCommand("gh-issue-read");
		const state = seedRegistry([def]);
		const { pi, tools, handlers } = recordingPi();
		registerDelegateTool(pi, state);
		registerToolCallAdvisor(pi, state);

		await tools[0].execute("call-allowed", { command: "mach12:gh-issue-read", args: "55" }, undefined, undefined, {
			cwd: "/",
		});

		const toolCallHandler = handlers.get("tool_call")![0] as any;
		await toolCallHandler({ type: "tool_call", toolCallId: "x", toolName: "bash", input: {} });

		expect(warnSpy).not.toHaveBeenCalled();
	});
});

// Proves index.ts actually wires registerBaseDirectives into the extension —
// the unit suite (base-directives.test.ts) covers the injector in isolation,
// but only the real default export catches a dropped registerBaseDirectives call in the factory.
// Loading the whole factory exercises the live registration order; emitting
// before_agent_start confirms the directives land on top of Pi's assembled
// prompt (the identity anchor is unique to the base directives, so its presence
// proves the injector ran).
describe("integration smoke — base directives wired into the extension factory", () => {
	it("scramjet() registers the injector so before_agent_start appends the directives", async () => {
		const { pi, handlers } = recordingPi();
		scramjet(pi);

		const beforeAgentStart = handlers.get("before_agent_start") ?? [];
		expect(beforeAgentStart.length).toBeGreaterThan(0);

		// Each before_agent_start handler is independent; the base-directives one
		// returns the appended prompt while the others (task-complete, delegate,
		// history) no-op for this empty state. Concatenate to find the directives.
		const outputs: string[] = [];
		for (const handler of beforeAgentStart) {
			const result = (await handler({ systemPrompt: "BASE PROMPT" })) as { systemPrompt?: string } | undefined;
			if (result?.systemPrompt) outputs.push(result.systemPrompt);
		}
		const combined = outputs.join("\n\n");

		expect(combined).toContain("BASE PROMPT");
		expect(combined).toContain("Scramjet is the harness you are running under");
	});
});

// S21: end-to-end chain smoke under /scramjet on. Exercises every harness
// module that participates in the dispatch loop — scramjet-command,
// history, command-status, auto-continue — against a synthetic two-command
// registry. The point isn't to re-test each module in isolation (every
// one already has its own suite) but to assert they compose through the
// two-phase command-status protocol (issue 84): the toggle writer flips
// state.enabled, the input handler records origin/activeTopLevelCommand and
// starts the running phase, the answer turn's agent_end defers a hidden
// status probe, the agent reports completion via scramjet_command_status, and
// auto-continue's forced-mode dispatch fires the next slash that the input
// handler then records as origin: "forced".
describe("integration smoke — end-to-end chain under /scramjet on (S21)", () => {
	beforeEach(() => vi.useFakeTimers());
	afterEach(() => vi.useRealTimers());

	interface RegisteredCommand {
		name: string;
		spec: { description?: string; handler: (args: string, ctx: unknown) => unknown };
	}

	function bigRecordingPi() {
		const handlers = new Map<string, ((event: unknown, ctx?: unknown) => unknown)[]>();
		const tools: any[] = [];
		const commands: RegisteredCommand[] = [];
		const appended: { type: string; data: unknown }[] = [];
		const probes: { message: any; options?: any }[] = [];
		const dispatched: { input: string; options?: any }[] = [];
		const pi: any = {
			isStreaming: false,
			on(event: string, handler: any) {
				const list = handlers.get(event) ?? [];
				list.push(handler);
				handlers.set(event, list);
			},
			registerTool(tool: any) {
				tools.push(tool);
			},
			registerCommand(name: string, spec: RegisteredCommand["spec"]) {
				commands.push({ name, spec });
			},
			appendEntry(type: string, data: unknown) {
				appended.push({ type, data });
			},
			// Hidden status-probe channel. A send issued while the run is still
			// streaming (i.e. synchronously from inside agent_end) would be dropped
			// by the real harness; model that so the deferral is exercised.
			sendMessage(message: any, options?: any) {
				if (pi.isStreaming) return;
				probes.push({ message, options });
			},
		};
		async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
			for (const h of handlers.get(event) ?? []) await h(payload, ctx);
		}
		return { pi, handlers, tools, commands, appended, probes, dispatched, emit };
	}

	function fakeCtx() {
		const notifications: { message: string; type?: string }[] = [];
		return {
			hasUI: false, // skip countdown widget so forced and closed both fire immediately
			ui: { notify: (m: string, t?: string) => notifications.push({ message: m, type: t }) },
			notifications,
		};
	}

	it("toggle on → user slash → answer turn → status probe → forced dispatch records origin: forced", async () => {
		const TARGET_BODY = "Run int:next now.";
		const origin: CommandDef = {
			name: "int:start",
			filePath: "/fake/int:start.md",
			body: "",
			next: { mode: "forced", target: "int:next" } as NextStepPolicy,
		};
		const target: CommandDef = { name: "int:next", filePath: "/fake/int:next.md", body: TARGET_BODY };
		const state: ScramjetState = freshState({
			registry: new Map([
				[origin.name, origin],
				[target.name, target],
			]),
			enabled: false, // user will flip this via /scramjet on
		});

		const bag = bigRecordingPi();
		const ctx: any = fakeCtx();
		ctx.dispatchUserInput = async (input: string, options?: any) => {
			bag.dispatched.push({ input, options });
			await bag.emit("input", { text: input, source: "extension" }, ctx);
		};

		// Wire every harness module that participates in a real dispatch.
		registerScramjetCommand(bag.pi, state);
		registerHistory(bag.pi, state);
		registerCommandStatusTool(bag.pi, state);
		registerAutoContinue(bag.pi, state);
		registerDelegateTool(bag.pi, state);
		registerToolCallAdvisor(bag.pi, state);

		// 1. User toggles /scramjet on — not a slash-command input event, that's
		//    Pi's command-handler path. We invoke the registered handler directly.
		const toggle = bag.commands.find((c) => c.name === "scramjet");
		expect(toggle).toBeDefined();
		await toggle?.spec.handler("on", ctx);
		expect(state.enabled).toBe(true);

		// 2. User types /int:start — the input handler records it as origin: "user",
		//    sets activeTopLevelCommand, and starts the running phase.
		await bag.emit("input", { text: "/int:start", source: "interactive" }, ctx);
		expect(state.activeTopLevelCommand).toBe("int:start");
		expect(state.commandPhase).toBe("running");
		expect(state.sidebarLog).toHaveLength(1);
		expect(state.sidebarLog[0].command).toBe("int:start");
		expect(state.sidebarLog[0].origin).toBe("user");

		// 3. The answer turn ends while the run is still streaming. auto-continue
		//    advances to the probing phase and DEFERS the hidden status probe; it
		//    must not send synchronously (that send would be dropped).
		bag.pi.isStreaming = true;
		await bag.emit("agent_end", {}, ctx);
		expect(state.commandPhase).toBe("probing");
		expect(bag.probes).toHaveLength(0);
		expect(bag.dispatched).toEqual([]);

		// 4. Once the run goes idle the probe fires and reaches the model.
		bag.pi.isStreaming = false;
		await vi.advanceTimersByTimeAsync(0);
		expect(bag.probes).toHaveLength(1);
		expect(bag.probes[0].message.display).toBe(false);
		expect(bag.probes[0].options).toEqual({ triggerTurn: true });

		// 5. The agent answers the probe by calling scramjet_command_status. int:start
		//    declares forced → int:next, so the probe turn's agent_end dispatches the
		//    slash wire through Pi's normal input path; history observes the
		//    extension-source input event and labels it origin: "forced".
		const statusTool = bag.tools.find((tool) => tool.name === "scramjet_command_status");
		expect(statusTool).toBeDefined();
		await statusTool.execute("status-call", { status: "completed", summary: "start complete" });
		expect(state.commandPhase).toBe("reported");
		await bag.emit("agent_end", {}, ctx);
		expect(bag.dispatched).toEqual([{ input: "/int:next", options: { deliverAs: "followUp" } }]);
		expect(state.pendingForcedDispatch).toBeNull();
		expect(state.commandPhase).toBe("running"); // int:next started its own answer turn

		// 6. The input event records the forced transition.
		expect(state.activeTopLevelCommand).toBe("int:next");
		expect(state.sidebarLog).toHaveLength(2);
		expect(state.sidebarLog[1].command).toBe("int:next");
		expect(state.sidebarLog[1].origin).toBe("forced");

		// 7. Journal entries reflect both transitions; the toggle entry also landed.
		const types = bag.appended.map((e) => e.type);
		expect(types).toContain("scramjet:enabled-toggle");
		expect(types.filter((t) => t === "scramjet:command-start")).toHaveLength(2);
	});
});
