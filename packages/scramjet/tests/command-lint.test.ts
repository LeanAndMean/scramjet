import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { lintCommandEntries } from "../src/command-lint.js";
import { runCommandLint } from "../src/command-lint-cli.js";
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
const sandboxes: string[] = [];

function commandSet(setName = "sample") {
	const sandbox = mkdtempSync(join(tmpdir(), "scramjet-command-lint-"));
	const setRoot = join(sandbox, setName);
	const commandsDir = join(setRoot, "commands");
	mkdirSync(commandsDir, { recursive: true });
	sandboxes.push(sandbox);
	return { setRoot, commandsDir };
}

function writeCommand(commandsDir: string, fileName: string, body = CLEAN_GOALS): string {
	const filePath = join(commandsDir, fileName);
	writeFileSync(filePath, `---\n---\n${body}`);
	return filePath;
}

function invokeCommandLint(args: string[]): { status: number; stdout: string; stderr: string } {
	let stdout = "";
	let stderr = "";
	const stdoutSpy = vi.spyOn(process.stdout, "write").mockImplementation((chunk) => {
		stdout += String(chunk);
		return true;
	});
	const stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation((chunk) => {
		stderr += String(chunk);
		return true;
	});
	try {
		return { status: runCommandLint(args), stdout, stderr };
	} finally {
		stdoutSpy.mockRestore();
		stderrSpy.mockRestore();
	}
}

afterEach(() => {
	vi.restoreAllMocks();
	for (const sandbox of sandboxes.splice(0)) rmSync(sandbox, { recursive: true, force: true });
});

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

describe("runCommandLint — targets, output, and exit policy", () => {
	it("accepts set roots, commands directories, and qualified files with deterministic direct-child ordering", () => {
		const { setRoot, commandsDir } = commandSet();
		const second = writeCommand(commandsDir, "sample:second.md");
		const first = writeCommand(commandsDir, "sample:first.md");
		writeFileSync(join(commandsDir, "notes.txt"), "ignored");
		mkdirSync(join(commandsDir, "nested"));
		writeCommand(join(commandsDir, "nested"), "sample:nested.md");

		const rootResult = invokeCommandLint(["--json", setRoot]);
		const directoryResult = invokeCommandLint(["--json", commandsDir]);
		const fileResult = invokeCommandLint(["--json", second]);
		const rootReport = JSON.parse(rootResult.stdout);
		const directoryReport = JSON.parse(directoryResult.stdout);
		const fileReport = JSON.parse(fileResult.stdout);

		expect(rootResult.status).toBe(0);
		expect(directoryResult.status).toBe(0);
		expect(fileResult.status).toBe(0);
		expect(rootReport.checkedFiles).toEqual([first, second]);
		expect(directoryReport).toEqual(rootReport);
		expect(fileReport.checkedFiles).toEqual([second]);
		expect(rootReport.summary).toEqual({ files: 2, errors: 0, warnings: 0 });
	});

	it("keeps warning strictness and runtime errors in the CLI exit policy", () => {
		const warningSet = commandSet("warning");
		writeCommand(warningSet.commandsDir, "warning:missing.md", "# Command\n\nWork.");
		const errorSet = commandSet("error");
		writeCommand(errorSet.commandsDir, "wrong.md");
		const incompleteRoot = mkdtempSync(join(tmpdir(), "scramjet-command-lint-incomplete-"));
		sandboxes.push(incompleteRoot);

		const defaultWarning = invokeCommandLint([warningSet.setRoot]);
		const strictWarning = invokeCommandLint(["--strict", warningSet.setRoot]);
		const runtimeError = invokeCommandLint([errorSet.setRoot]);
		const invalidTarget = invokeCommandLint([join(errorSet.setRoot, "missing")]);
		const incompleteTarget = invokeCommandLint([incompleteRoot]);

		expect(defaultWarning.status).toBe(0);
		expect(strictWarning.status).toBe(1);
		expect(defaultWarning.stdout).toBe(strictWarning.stdout);
		expect(runtimeError.status).toBe(1);
		expect(runtimeError.stdout).toContain("error runtime-unrecognized");
		expect(invalidTarget.status).toBe(2);
		expect(invalidTarget.stderr).toContain("missing");
		expect(incompleteTarget.status).toBe(2);
		expect(incompleteTarget.stderr).toContain("commands");
	});

	it("renders deterministic terminal-safe human and JSON reports", () => {
		const { commandsDir } = commandSet();
		const filePath = writeCommand(commandsDir, "sample:\u009bunsafe.md", "# Command\n\nWork.");

		const firstHuman = invokeCommandLint([filePath]);
		const secondHuman = invokeCommandLint([filePath]);
		const firstJson = invokeCommandLint(["--json", filePath]);
		const secondJson = invokeCommandLint(["--json", filePath]);

		expect(firstHuman).toEqual(secondHuman);
		expect(firstJson).toEqual(secondJson);
		expect(firstHuman.stdout).not.toContain("\u009b");
		expect(firstHuman.stdout).toContain("\\u009b");
		expect(firstJson.stdout).not.toContain("\u009b");
		expect(firstJson.stdout).toContain("\\u009b");
		expect(JSON.parse(firstJson.stdout).diagnostics[0].code).toBe("goals-missing");
	});

	it("does not change source bytes or directory entries", () => {
		const { setRoot, commandsDir } = commandSet();
		const filePath = writeCommand(commandsDir, "sample:unchanged.md", "# Command\r\n\r\nWork.\r\n");
		const beforeSource = readFileSync(filePath);
		const beforeEntries = readdirSync(commandsDir);

		expect(invokeCommandLint([setRoot]).status).toBe(0);
		expect(readFileSync(filePath)).toEqual(beforeSource);
		expect(readdirSync(commandsDir)).toEqual(beforeEntries);
	});

	it("strict-lints both bundled command sets", () => {
		const mach12 = invokeCommandLint(["--strict", "--json", resolve(__dirname, "../mach12")]);
		const scramjet = invokeCommandLint(["--strict", "--json", resolve(__dirname, "../scramjet")]);

		expect(mach12.status).toBe(0);
		expect(scramjet.status).toBe(0);
		expect(JSON.parse(mach12.stdout).summary).toEqual({ files: 19, errors: 0, warnings: 0 });
		expect(JSON.parse(scramjet.stdout).summary).toEqual({ files: 1, errors: 0, warnings: 0 });
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
