import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { parseCommandFile } from "../commands/loader.ts";
import { registerDelegateTool } from "../delegate.ts";
import { registerToolCallAdvisor } from "../tool-scope-advisory.ts";
import type { CommandDef, ScramjetState } from "../types.ts";
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
		expect(message).toContain("depth=0");
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
