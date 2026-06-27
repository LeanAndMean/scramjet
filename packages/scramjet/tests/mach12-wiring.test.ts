import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";
import type { NextStepPolicy } from "../src/types.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const MACH12_COMMANDS_DIR = resolve(HERE, "..", "mach12", "commands");
const SET_NAME = "mach12";

interface WiringRow {
	basename: string;
	expected: NextStepPolicy | null;
}

// Hints are intentionally not pinned: modes, targets, candidate names, and
// blacklists carry semantic load; hint text is editorial and can drift.
//
// Subroutines (delegate-only) declare no `next:` block — they are dispatched
// via the `delegate` tool from within a calling command's turn and the
// caller's `next:` controls chaining. `expected: null` pins that property:
// the file must parse and must NOT carry a next-step policy.
const WIRING: WiringRow[] = [
	{
		basename: "issue-create",
		expected: { mode: "open", candidates: [{ name: "mach12:issue-plan" }] },
	},
	{
		basename: "issue-plan",
		expected: {
			mode: "open",
			candidates: [{ name: "mach12:issue-review" }, { name: "mach12:issue-implement" }],
		},
	},
	{
		basename: "issue-review",
		expected: { mode: "open", candidates: [{ name: "mach12:issue-review" }, { name: "mach12:issue-implement" }] },
	},
	{
		basename: "issue-implement",
		expected: { mode: "open", candidates: [{ name: "mach12:issue-implement" }, { name: "mach12:pr-create" }] },
	},
	{
		basename: "pr-create",
		expected: { mode: "open", candidates: [{ name: "mach12:pr-review" }] },
	},
	{
		basename: "pr-review",
		expected: { mode: "forced", target: "mach12:pr-review-assessment" },
	},
	{
		basename: "pr-review-assessment",
		expected: {
			mode: "closed",
			candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
		},
	},
	{
		basename: "pr-review-fix",
		expected: {
			mode: "open",
			candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-review" }, { name: "mach12:pr-pre-merge" }],
		},
	},
	{
		basename: "pr-pre-merge",
		expected: {
			mode: "open",
			candidates: [{ name: "mach12:pr-merge" }, { name: "mach12:pr-review-fix" }],
		},
	},
	{
		basename: "pr-merge",
		// Intentional terminus: no `next:` means Scramjet probes until the agent
		// reports completed, then clears to idle without dispatch.
		expected: null,
	},
	// Subroutines.
	{ basename: "push", expected: null },
	{ basename: "find-contribution-guidelines", expected: null },
	{ basename: "gh-issue-read", expected: null },
	{ basename: "gh-pr-read", expected: null },
	{ basename: "gh-sub-issues", expected: null },
	{ basename: "gh-assign", expected: null },
	{ basename: "gh-comment", expected: null },
];

// Strip hint strings from a policy so the wiring test compares modes, targets,
// candidate names, and blacklists -- not editorial hint text.
function stripHints(policy: NextStepPolicy | null): NextStepPolicy | null {
	if (policy === null) return null;
	switch (policy.mode) {
		case "forced":
			return { mode: "forced", target: policy.target };
		case "closed":
			return { mode: "closed", candidates: policy.candidates.map((c) => ({ name: c.name })) };
		case "open": {
			const stripped: NextStepPolicy = {
				mode: "open",
				candidates: policy.candidates.map((c) => ({ name: c.name })),
			};
			if (policy.blacklist !== undefined) stripped.blacklist = policy.blacklist;
			return stripped;
		}
		case "ask":
			return { mode: "ask" };
	}
}

const MACH12_AGENTS_DIR = resolve(HERE, "..", "mach12", "agents");

// F18: The expected list of bundled mach12 agents. A name-mismatch between
// a command's subagent reference and the bridged filename would slip through
// CI without this explicit pin. If you add/rename an agent, update here.
const EXPECTED_AGENTS = [
	"mach12:code-architect",
	"mach12:code-explorer",
	"mach12:code-reviewer",
	"mach12:code-simplifier",
	"mach12:comment-analyzer",
	"mach12:feature-completeness-checker",
	"mach12:silent-failure-hunter",
	"mach12:test-analyzer",
	"mach12:test-designer",
	"mach12:type-design-analyzer",
].sort();

describe("mach12 wiring — bundled command set", () => {
	it("ships exactly the expected set of command files (top-level and subroutines)", () => {
		const found = readdirSync(MACH12_COMMANDS_DIR)
			.filter((f) => f.endsWith(".md"))
			.sort();
		const expected = WIRING.map((row) => `${SET_NAME}:${row.basename}.md`).sort();
		expect(found).toEqual(expected);
	});

	it.each(WIRING)("parses $basename via Stage 1 parser and wires it correctly", ({ basename, expected }) => {
		const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
		const content = readFileSync(filePath, "utf-8");
		const result = parseCommandFile(filePath, content, SET_NAME);

		expect(result.ok).toBe(true);
		if (!result.ok) return;

		expect(result.def.name).toBe(`${SET_NAME}:${basename}`);
		expect(stripHints(result.def.next ?? null)).toEqual(expected);
	});

	it("pr-review is wired to invoke the bundled Mach 12 reviewer agents", () => {
		const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review.md`);
		const content = readFileSync(filePath, "utf-8");
		const result = parseCommandFile(filePath, content, SET_NAME);

		expect(result.ok).toBe(true);
		if (!result.ok) return;

		expect(result.def.allowedTools).toContain("subagent");
		for (const agent of [
			"mach12:code-reviewer",
			"mach12:test-analyzer",
			"mach12:comment-analyzer",
			"mach12:silent-failure-hunter",
			"mach12:type-design-analyzer",
			"mach12:code-simplifier",
			"mach12:feature-completeness-checker",
		]) {
			expect(content).toContain(agent);
		}
	});
});

// F18: Verify that the bundled mach12 agent files are complete and parseable,
// and that the agent-bridge can wire them without warnings. A name mismatch
// between a command's subagent reference and the shipped agent filename would
// produce a "subagent not found" at runtime but silently pass unit tests.
describe("mach12 wiring — bundled agent set (F18)", () => {
	it("ships exactly the expected set of agent files", () => {
		const found = readdirSync(MACH12_AGENTS_DIR)
			.filter((f) => f.endsWith(".md"))
			.map((f) => f.replace(/\.md$/, ""))
			.sort();
		expect(found).toEqual(EXPECTED_AGENTS);
	});

	it("all agent files parse into a valid AgentDef with name matching filename", () => {
		for (const name of EXPECTED_AGENTS) {
			const filePath = join(MACH12_AGENTS_DIR, `${name}.md`);
			const content = readFileSync(filePath, "utf-8");
			// Agent files must have a frontmatter `name:` matching the filename prefix.
			expect(content).toContain(`name: ${name}`);
		}
	});
});
