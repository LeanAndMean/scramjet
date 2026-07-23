import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { buildCommandExpansion, extractArgs, registerHistory } from "../src/history.js";
import type { CommandDef, CommandRegistry } from "../src/types.js";
import { freshState, recordingPi } from "./helpers.js";

function def(name: string, body: string): CommandDef {
	return { name, filePath: `/fake/${name}.md`, body };
}

describe("extractArgs", () => {
	it("returns empty string for command with no args", () => {
		expect(extractArgs("/mach12:push")).toBe("");
	});

	it("returns args after first space", () => {
		expect(extractArgs("/mach12:issue-plan 82")).toBe("82");
	});

	it("preserves multi-word args", () => {
		expect(extractArgs("/mach12:issue-create fix the login bug")).toBe("fix the login bug");
	});

	it("handles leading whitespace in args", () => {
		expect(extractArgs("/mach12:push  extra spaces")).toBe(" extra spaces");
	});

	it("handles tab-separated args", () => {
		expect(extractArgs("/mach12:push\tcontext")).toBe("context");
	});
});

describe("buildCommandExpansion", () => {
	it("wraps body in scramjet-command tags with substituted args", () => {
		const d = def("mach12:push", "# Push\n\n<caller-context>\n$ARGUMENTS\n</caller-context>\n\nDo the thing.");
		const result = buildCommandExpansion("mach12:push", d, "stage 1 summary");
		expect(result).toBe(
			'<scramjet-command name="mach12:push">\n# Push\n\n<caller-context>\nstage 1 summary\n</caller-context>\n\nDo the thing.\n</scramjet-command>',
		);
	});

	it("handles empty args", () => {
		const d = def("mach12:push", "# Push\n\n<caller-context>\n$ARGUMENTS\n</caller-context>");
		const result = buildCommandExpansion("mach12:push", d, "");
		expect(result).toBe(
			'<scramjet-command name="mach12:push">\n# Push\n\n<caller-context>\n\n</caller-context>\n</scramjet-command>',
		);
	});

	it("substitutes positional args", () => {
		const d = def("test:cmd", "Issue $1 stage $2");
		const result = buildCommandExpansion("test:cmd", d, "82 3");
		expect(result).toBe('<scramjet-command name="test:cmd">\nIssue 82 stage 3\n</scramjet-command>');
	});

	it("does not double-wrap if body already has scramjet-command tags", () => {
		const body = '<scramjet-command name="test:cmd">\n# Already wrapped\n$ARGUMENTS\n</scramjet-command>';
		const d = def("test:cmd", body);
		const result = buildCommandExpansion("test:cmd", d, "args");
		expect(result).toBe('<scramjet-command name="test:cmd">\n# Already wrapped\nargs\n</scramjet-command>');
	});

	it("handles command with no user-context or caller-context", () => {
		const d = def("mach12:find-contribution-guidelines", "# Find Guidelines\n\nSearch for CONTRIBUTING.md.");
		const result = buildCommandExpansion("mach12:find-contribution-guidelines", d, "");
		expect(result).toBe(
			'<scramjet-command name="mach12:find-contribution-guidelines">\n# Find Guidelines\n\nSearch for CONTRIBUTING.md.\n</scramjet-command>',
		);
	});
});

describe("input handler — expansion transform", () => {
	function registryOf(defs: CommandDef[]): CommandRegistry {
		return new Map(defs.map((d) => [d.name, d]));
	}

	async function fireInput(
		registry: CommandRegistry,
		text: string,
		source: "interactive" | "extension" = "interactive",
	) {
		const state = freshState({ registry });
		const { pi, handlers } = recordingPi();
		registerHistory(pi, state);
		const inputHandlers = handlers.get("input") ?? [];
		let result: unknown;
		for (const h of inputHandlers) {
			result = await h({ text, source });
		}
		return { state, result };
	}

	it("returns transform with wrapped body for a registered command", async () => {
		const d = def("mach12:push", "# Push\n\n<caller-context>\n$ARGUMENTS\n</caller-context>");
		const { result } = await fireInput(registryOf([d]), "/mach12:push stage 1");
		expect(result).toEqual({
			action: "transform",
			text: '<scramjet-command name="mach12:push">\n# Push\n\n<caller-context>\nstage 1\n</caller-context>\n</scramjet-command>',
		});
	});

	it("returns undefined for non-registered input", async () => {
		const d = def("mach12:push", "body");
		const { result } = await fireInput(registryOf([d]), "just chatting");
		expect(result).toBeUndefined();
	});

	it("returns undefined for unregistered slash commands", async () => {
		const d = def("mach12:push", "body");
		const { result } = await fireInput(registryOf([d]), "/unknown-cmd");
		expect(result).toBeUndefined();
	});

	it("records command start before returning transform", async () => {
		const d = def("mach12:push", "body");
		const { state, result } = await fireInput(registryOf([d]), "/mach12:push args");
		expect(result).toHaveProperty("action", "transform");
		expect(state.lifecycle.activeCommand).toBe("mach12:push");
	});
});

describe("static command framing validation", () => {
	const commandDirs = [join(__dirname, "../mach12/commands"), join(__dirname, "../scramjet/commands")];
	const commands = commandDirs.flatMap((dir) =>
		readdirSync(dir)
			.filter((file) => file.endsWith(".md"))
			.map((file) => ({ file, content: readFileSync(join(dir, file), "utf-8") })),
	);

	it("no bundled command contains static scramjet-command tags", () => {
		for (const { file, content } of commands) {
			expect(content, `${file} still contains <scramjet-command`).not.toMatch(/<scramjet-command/);
			expect(content, `${file} still contains </scramjet-command`).not.toMatch(/<\/scramjet-command>/);
		}
	});

	it("all contextual commands contain one framed ARGUMENTS expansion", () => {
		const commandsWithContext = commands.filter(({ file }) => file !== "mach12:find-contribution-guidelines.md");
		for (const { file, content } of commandsWithContext) {
			const contextBlock = content.match(/<(user|caller)-context>[\s\S]*?<\/(user|caller)-context>/)?.[0];
			expect(contextBlock, `${file} is missing context tags`).toBeDefined();
			expect(content.match(/\$ARGUMENTS/g), `${file} must expand arguments exactly once`).toHaveLength(1);
			expect(contextBlock, `${file} must frame its argument expansion`).toContain("$ARGUMENTS");
		}
	});
});
