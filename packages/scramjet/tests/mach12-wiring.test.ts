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
	delegateOnly?: true;
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
	// Subroutines (delegate-only).
	{ basename: "push", expected: null, delegateOnly: true },
	{ basename: "find-contribution-guidelines", expected: null, delegateOnly: true },
	{ basename: "gh-issue-read", expected: null, delegateOnly: true },
	{ basename: "gh-pr-read", expected: null, delegateOnly: true },
	{ basename: "gh-sub-issues", expected: null, delegateOnly: true },
	{ basename: "gh-assign", expected: null, delegateOnly: true },
	{ basename: "gh-comment", expected: null, delegateOnly: true },
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
	"mach12:independent-assessor",
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

	it.each(WIRING)(
		"parses $basename via Stage 1 parser and wires it correctly",
		({ basename, expected, delegateOnly }) => {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const content = readFileSync(filePath, "utf-8");
			const result = parseCommandFile(filePath, content, SET_NAME);

			expect(result.ok).toBe(true);
			if (!result.ok) return;

			expect(result.def.name).toBe(`${SET_NAME}:${basename}`);
			expect(stripHints(result.def.next ?? null)).toEqual(expected);
			if (delegateOnly) {
				expect(result.def.delegateOnly).toBe(true);
			} else {
				expect(result.def.delegateOnly).toBeUndefined();
			}
		},
	);

	// Issue 278: top-level command bodies teach evidence-first status reporting
	// (summary before status) and no longer carry the retired "When Scramjet asks…"
	// timing incantation. This is a semantic check, not a snapshot of the
	// command-specific next-step prose (which stays free to drift).
	it.each(WIRING.filter((row) => !row.delegateOnly))(
		"$basename teaches answer-first, incremental-summary status reporting",
		({ basename }) => {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const content = readFileSync(filePath, "utf-8");
			expect(content).not.toContain("When Scramjet asks");
			expect(content).toContain("After delivering your answer");
			expect(content).toContain("summarize the work you performed in `summary`");
		},
	);

	it("routes both independent-assessment steps to mach12:independent-assessor with no dangling general-purpose reference", () => {
		for (const basename of ["pr-review-assessment", "issue-review"]) {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const content = readFileSync(filePath, "utf-8");
			expect(content).toContain("mach12:independent-assessor");
			expect(content).not.toContain("general-purpose subagent");

			const result = parseCommandFile(filePath, content, SET_NAME);
			expect(result.ok).toBe(true);
			if (!result.ok) return;
			expect(result.def.allowedTools).toContain("subagent");
		}
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

describe("mach12 standard PR linkage", () => {
	const activeCommands = readdirSync(MACH12_COMMANDS_DIR)
		.filter((file) => file.endsWith(".md"))
		.map((file) => [file, readFileSync(join(MACH12_COMMANDS_DIR, file), "utf-8")] as const);
	const prCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-create.md`), "utf-8");

	it("retires delivery identity from every active command", () => {
		for (const [file, content] of activeCommands) {
			expect(content, file).not.toContain("mach12:gh-delivery-unit");
			expect(content, file).not.toContain("<!-- mach12-pr -->");
			expect(content, file).not.toContain("mach12-delivery-unit-v1");
			expect(content, file).not.toContain("Delivery-unit:");
		}
	});

	it("resolves ambiguous linkage before drafting", () => {
		const ambiguity = prCreate.indexOf("Resolve issue-linkage ambiguity before constructing a draft");
		const draft = prCreate.indexOf("## Step 3: Draft PR and get approval");
		expect(ambiguity).toBeGreaterThan(-1);
		expect(ambiguity).toBeLessThan(draft);
		expect(prCreate).toContain("could be either an issue number or general context");
		expect(prCreate).toContain("multiple plausible issue candidates");
		expect(prCreate).toContain("select exactly one issue or explicitly decline linkage");
	});

	it("proposes zero or one closer without relationship expansion", () => {
		expect(prCreate).toContain("exactly one standalone `Fixes #N` line");
		expect(prCreate).toContain("Zero closing-keyword lines");
		expect(prCreate).toContain("at most one proposed closer");
		expect(prCreate).not.toContain("mach12:gh-sub-issues");
		expect(prCreate).not.toContain("close-set");
	});

	it("preserves full-body approval and validates final linkage cardinality", () => {
		expect(prCreate).toContain("Before presenting any initial or modified complete body");
		expect(prCreate).toContain("zero or one closing-keyword occurrence");
		expect(prCreate).toContain("standalone line containing exactly one issue target");
		expect(prCreate).toContain("reject a line with multiple targets, multiple closing keywords");
		expect(prCreate).toContain("revalidate the displayed complete body");
		expect(prCreate).toContain("Immediately before creation, validate the final approved body once more");
		expect(prCreate.indexOf("Immediately before creation")).toBeLessThan(prCreate.indexOf("gh pr create"));
		expect(prCreate).toContain("Present the validated title and complete body");
		expect(prCreate).toContain("Approve, Modify, or Cancel");
		expect(prCreate).toContain("<approved-body>");
		expect(prCreate).toContain('Report `status: "incomplete"` if the user cancelled');
	});
});

describe("mach12 ordinary PR readiness", () => {
	const preMerge = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-pre-merge.md`), "utf-8");
	const merge = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-merge.md`), "utf-8");
	const readinessSection = (content: string) =>
		content.slice(content.indexOf("## Step 2:"), content.indexOf("## Step 3:"));

	it.each([
		["pr-pre-merge", preMerge],
		["pr-merge", merge],
	])("%s checks ordinary readiness in safety order", (name, content) => {
		const readiness = readinessSection(content);
		const predicates = [
			"`state` is not `OPEN`",
			"`isDraft` is `true`",
			"`CHANGES_REQUESTED`",
			"`REVIEW_REQUIRED`",
			"gh pr checks <pr-number> --required --json name,state,bucket,link",
			"Classify `mergeStateStatus` exhaustively",
			"`mergeable` is `CONFLICTING`",
			"one bounded reread",
		];
		for (let index = 0; index < predicates.length; index++) {
			expect(readiness.indexOf(predicates[index]), `${name}: ${predicates[index]}`).toBeGreaterThan(
				index === 0 ? -1 : readiness.indexOf(predicates[index - 1]),
			);
		}
		expect(readiness).toContain("Empty or null `reviewDecision` is not blocking by itself");
		expect(content.indexOf(name === "pr-merge" ? "gh pr merge" : "gh pr checkout <pr-number>")).toBeGreaterThan(
			content.indexOf("## Step 3:"),
		);
	});

	it.each([
		["pr-pre-merge", preMerge],
		["pr-merge", merge],
	])("%s defines exhaustive check buckets and merge states", (_name, content) => {
		const readiness = readinessSection(content);
		for (const state of ["`CLEAN`", "`HAS_HOOKS`", "`UNSTABLE`", "`BLOCKED`", "`BEHIND`", "`DIRTY`", "`UNKNOWN`"]) {
			expect(readiness).toContain(state);
		}
		expect(readiness).toContain("no required checks reported on the '<branch>' branch");
		expect(readiness).toContain("any other nonzero exit");
	});

	it("pre-merge routes remediable initial outcomes to later steps", () => {
		const readiness = readinessSection(preMerge);
		expect(readiness).toContain("`pass` and `skipping` as settled and nonfailing");
		expect(readiness).toContain("record `pending` for Step 9 to wait on");
		expect(readiness).toContain("record `fail` or `cancel` for Step 9 to diagnose and repair");
		expect(readiness).toContain("`UNSTABLE` may continue only because Step 9 must repair or resolve CI");
		expect(readiness).toContain("`BLOCKED` may continue only when Step 5 found a `pending`, `fail`, or `cancel`");
	});

	it("merge fails closed on unsettled checks and non-ready states", () => {
		const readiness = readinessSection(merge);
		expect(readiness).toContain("Buckets `pass` and `skipping` are settled and nonfailing");
		expect(readiness).toContain("any `pending`, `fail`, or `cancel` bucket is blocked");
		expect(readiness).toContain("`CLEAN` and `HAS_HOOKS` are ready");
		expect(readiness).toContain("`UNSTABLE`, `BLOCKED`, and `DRAFT` are blocked");
		expect(readiness).toContain("`DIRTY` is a confirmed conflict and blocked");
	});

	it.each([
		["pr-pre-merge", preMerge],
		["pr-merge", merge],
	])("%s bounds indeterminate readiness and offers no bypass", (_name, content) => {
		expect(readinessSection(content)).toContain("one bounded reread");
		expect(content).toContain("still indeterminate");
		expect(content).not.toMatch(/--force|--admin/);
	});

	it("pre-merge stops when checklist changes cannot reach the PR", () => {
		const commitSection = preMerge.slice(preMerge.indexOf("## Step 8:"), preMerge.indexOf("## Step 9:"));
		const stageClause = commitSection.slice(
			commitSection.indexOf("2. **Stage**"),
			commitSection.indexOf("3. **Commit**"),
		);
		const commitClause = commitSection.slice(
			commitSection.indexOf("3. **Commit**"),
			commitSection.indexOf("4. **Push**"),
		);
		const pushClause = commitSection.slice(commitSection.indexOf("4. **Push**"));
		for (const clause of [stageClause, commitClause, pushClause]) {
			expect(clause).toContain("report the command incomplete");
			expect(clause).toContain("stop before CI and final readiness");
		}
		expect(commitSection).toContain("Step 9 may begin only after a successful push");

		const finalSection = preMerge.slice(preMerge.indexOf("## Step 10:"));
		expect(finalSection).toContain("After all checklist changes are pushed and CI settles");
		expect(finalSection).toContain("final authoritative readiness reread");
	});

	it("pre-merge defines terminal status predicates and requires final readiness", () => {
		expect(preMerge).toContain('Report `status: "completed"` only');
		expect(preMerge).toContain('Report `status: "blocked"`');
		expect(preMerge).toContain('Report `status: "incomplete"`');
		expect(preMerge).toContain("final authoritative readiness reread");
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
