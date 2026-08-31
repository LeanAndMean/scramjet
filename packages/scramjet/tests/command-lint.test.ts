import { describe, expect, it } from "vitest";
import { lintCommandEntries } from "../src/command-lint.js";
import { buildRegistry, type FileEntry } from "../src/commands/loader.js";

function entry(file: string, body: string, frontmatter = "", scope: "global" | "project" = "global"): FileEntry {
	return {
		filePath: `/sets/sample/commands/${file}`,
		content: `---\n${frontmatter}---\n${body}`,
		setName: "sample",
		scope,
	};
}

const CLEAN_GOALS = "# Command\n\n## Goals\n\n- Deliver a durable result.\n\n## Step 1: Work\n\nDo the work.";

describe("lintCommandEntries — runtime-derived diagnostics", () => {
	it("maps runtime rejection to an error while retaining readable Markdown warnings", () => {
		const input = entry("wrong.md", "# Command\n\nDo the work.");
		const result = lintCommandEntries([input]);

		expect(result.registry.size).toBe(0);
		expect(result.diagnostics).toEqual([
			{
				code: "runtime-unrecognized",
				severity: "error",
				filePath: input.filePath,
				message: expect.stringContaining('filename must start with "sample:"'),
			},
			{
				code: "goals-missing",
				severity: "warning",
				filePath: input.filePath,
				message: expect.stringContaining("## Goals"),
			},
		]);
	});

	it("maps shadowing to an error naming the exact runtime winner", () => {
		const winner = entry("sample:same.md", CLEAN_GOALS);
		const shadowed = entry("sample:same.md", CLEAN_GOALS, "", "project");
		const result = lintCommandEntries([winner, shadowed]);

		expect(result.registry.get("sample:same")?.filePath).toBe(winner.filePath);
		expect(result.diagnostics).toEqual([
			{
				code: "runtime-shadowed",
				severity: "error",
				filePath: shadowed.filePath,
				message: `Command sample:same is shadowed by ${winner.filePath}.`,
			},
		]);
	});

	it("maps tolerated runtime notices to warnings", () => {
		const input = entry("sample:notice.md", CLEAN_GOALS, "delegate-only: false\nallowed-tools: [Read, 42]\n");
		const result = lintCommandEntries([input]);

		expect(result.registry.has("sample:notice")).toBe(true);
		expect(result.diagnostics.map(({ code, severity }) => ({ code, severity }))).toEqual([
			{ code: "runtime-notice", severity: "warning" },
			{ code: "runtime-notice", severity: "warning" },
		]);
		expect(result.diagnostics[0].message).toContain("delegate-only");
		expect(result.diagnostics[1].message).toContain("allowed-tools");
	});
});

describe("lintCommandEntries — Goals structure", () => {
	it("accepts one early non-empty Goals list", () => {
		expect(lintCommandEntries([entry("sample:clean.md", CLEAN_GOALS)]).diagnostics).toEqual([]);
	});

	it("reports a missing Goals section", () => {
		const diagnostics = lintCommandEntries([
			entry("sample:missing.md", "# Command\n\n## Step 1\n\nWork."),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-missing"]);
	});

	it("does not accept closing hashes without separating whitespace", () => {
		const diagnostics = lintCommandEntries([
			entry("sample:malformed-heading.md", "# Command\n\n## Goals#\n\n- Not a Goals section."),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-missing"]);
	});

	it("reports every duplicate Goals heading after the first", () => {
		const input = entry(
			"sample:duplicate.md",
			"# Command\n\n## Goals\n\n- First.\n\n## Goals\n\n- Second.\n\n## Goals\n\n- Third.",
		);
		const diagnostics = lintCommandEntries([input]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-duplicate", "goals-duplicate"]);
		expect(diagnostics.map((diagnostic) => diagnostic.line)).toEqual([9, 13]);
	});

	it("reports empty Goals", () => {
		const diagnostics = lintCommandEntries([
			entry("sample:empty.md", "# Command\n\n## Goals\n\n## Step 1\n\nWork."),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-empty"]);
	});

	it("checks malformed duplicate Goals sections independently", () => {
		const diagnostics = lintCommandEntries([
			entry("sample:duplicate-empty.md", "# Command\n\n## Goals\n\n- First.\n\n## Goals\n\n## Step 1\n\nWork."),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-duplicate", "goals-empty"]);
	});

	it.each(["Step 1: Start", "Steps"])("reports Goals placed after a procedural ## %s heading", (heading) => {
		const diagnostics = lintCommandEntries([
			entry("sample:late.md", `# Command\n\n## ${heading}\n\nWork.\n\n## Goals\n\n- Finish.`),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-order"]);
	});

	it("reports Goals content without Markdown list items", () => {
		const diagnostics = lintCommandEntries([
			entry("sample:prose.md", "# Command\n\n## Goals\n\nDeliver a durable result."),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-list"]);
	});

	it("reports argument substitution and context embedded in Goals", () => {
		const diagnostics = lintCommandEntries([
			entry(
				"sample:embedded.md",
				"# Command\n\n## Goals\n\n- Process <user-context>$ARGUMENTS</user-context>.\n\n## Step 1\n\nWork.",
				'argument-hint: "<input>"\n',
			),
		]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["goals-context"]);
	});

	it("ignores headings, list items, substitutions, and context tags inside fences", () => {
		const input = entry(
			"sample:fences.md",
			"# Command\n\n## Goals\n\n- Deliver a durable result.\n\n```markdown\n## Goals\n- $ARGUMENTS\n<caller-context>\n```\n\n~~~markdown\n## Step 0\n~~~\n\n## Step 1\n\nWork.",
		);
		expect(lintCommandEntries([input]).diagnostics).toEqual([]);
	});
});

describe("lintCommandEntries — argument framing", () => {
	it("accepts one top-level substitution in user context", () => {
		const input = entry(
			"sample:top.md",
			`${CLEAN_GOALS}\n\n<user-context>\n$ARGUMENTS\n</user-context>`,
			'argument-hint: "<input>"\n',
		);
		expect(lintCommandEntries([input]).diagnostics).toEqual([]);
	});

	it("accepts one delegated substitution in caller context", () => {
		const input = entry(
			"sample:delegate.md",
			`${CLEAN_GOALS}\n\n<caller-context>\n$1\n</caller-context>`,
			'argument-hint: "<input>"\ndelegate-only: true\n',
		);
		expect(lintCommandEntries([input]).diagnostics).toEqual([]);
	});

	it("reports role-inappropriate context framing", () => {
		const input = entry(
			"sample:wrong-context.md",
			`${CLEAN_GOALS}\n\n<caller-context>\n$ARGUMENTS\n</caller-context>`,
			'argument-hint: "<input>"\n',
		);
		const diagnostics = lintCommandEntries([input]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["context-framing"]);
		expect(diagnostics[0].message).toContain("<user-context>");
	});

	it("reports missing and repeated substitutions with one framing diagnostic", () => {
		const missing = entry("sample:no-context.md", CLEAN_GOALS, 'argument-hint: "<input>"\n');
		const repeated = entry(
			"sample:repeated.md",
			`${CLEAN_GOALS}\n\n<user-context>\n$ARGUMENTS $1\n</user-context>`,
			'argument-hint: "<input>"\n',
		);
		const diagnostics = lintCommandEntries([missing, repeated]).diagnostics;
		expect(diagnostics.map((diagnostic) => diagnostic.code)).toEqual(["context-framing", "context-framing"]);
	});

	it("allows commands with no arguments to omit context framing", () => {
		expect(lintCommandEntries([entry("sample:no-args.md", CLEAN_GOALS)]).diagnostics).toEqual([]);
	});
});

describe("lintCommandEntries — purity and ordering", () => {
	it("preserves registry behavior for warning-only authoring findings", () => {
		const inputs = [entry("sample:missing.md", "# Missing Goals"), entry("sample:clean.md", CLEAN_GOALS)];
		const runtime = buildRegistry(inputs);
		const lint = lintCommandEntries(inputs);

		expect([...lint.registry.entries()]).toEqual([...runtime.registry.entries()]);
		expect(lint.registry.size).toBe(2);
		expect(lint.diagnostics).toHaveLength(1);
		expect(lint.diagnostics[0].severity).toBe("warning");
	});

	it("orders diagnostics by input, then runtime facts, then authoring rules", () => {
		const inputs = [
			entry("wrong.md", "# Missing Goals"),
			entry("sample:notice.md", "# Missing Goals", "delegate-only: false\n"),
		];
		const first = lintCommandEntries(inputs).diagnostics;
		const second = lintCommandEntries(inputs).diagnostics;

		expect(first).toEqual(second);
		expect(first.map((diagnostic) => diagnostic.code)).toEqual([
			"runtime-unrecognized",
			"goals-missing",
			"runtime-notice",
			"goals-missing",
		]);
	});

	it("preserves every source byte and input field", () => {
		const inputs = [
			entry("sample:bom.md", `﻿${CLEAN_GOALS}\r\n`, "description: Preserve me\n"),
			entry("sample:other.md", "# Command\n\nNo Goals."),
		];
		const before = structuredClone(inputs);
		lintCommandEntries(inputs);
		expect(inputs).toEqual(before);
	});
});
