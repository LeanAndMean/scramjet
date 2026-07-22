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
	{ basename: "gh-delivery-unit", expected: null, delegateOnly: true },
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

describe("mach12 delivery-unit linkage contract", () => {
	const deliveryUnitPath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:gh-delivery-unit.md`);
	const deliveryUnit = readFileSync(deliveryUnitPath, "utf-8");

	it("is a tightly scoped delegate-only subroutine", () => {
		const result = parseCommandFile(deliveryUnitPath, deliveryUnit, SET_NAME);
		expect(result.ok).toBe(true);
		if (!result.ok) return;

		expect(result.def.delegateOnly).toBe(true);
		expect(result.def.allowedTools).toEqual(["bash"]);
		expect(result.def.next).toBeUndefined();
		expect(deliveryUnit.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(deliveryUnit).not.toContain("report_scramjet_command_status");
	});

	it("pins exact linked and explicit-none identity with universal absent-identity hold", () => {
		expect(deliveryUnit).toContain("<!-- mach12-pr -->\n<!-- mach12-delivery-unit-v1 -->\nDelivery-unit: #<D>");
		expect(deliveryUnit).toContain("<!-- mach12-pr -->\n<!-- mach12-delivery-unit-v1 -->\nDelivery-unit: none");
		expect(deliveryUnit).toContain("Absent identity always returns `verdict: hold`");
		expect(deliveryUnit).toContain("Identity without exact provenance also holds");
		expect(deliveryUnit).toContain("zero actual `closingIssuesReferences`");
		expect(deliveryUnit).toContain("zero standalone closing-keyword lines");
		expect(deliveryUnit).toContain("zero standalone `Part of #<number>` lines");
		expect(deliveryUnit).toContain("There is no unlinked representation other than exact `Delivery-unit: none`");
		expect(deliveryUnit).not.toContain("verdict: not-applicable");
	});

	it("requires informed manual migration without inference or body mutation", () => {
		expect(deliveryUnit).toContain("legacy or external PR");
		expect(deliveryUnit).toContain("inspect the intended delivery scope before repairing or redrafting");
		expect(deliveryUnit).toContain("Never infer identity from existing closers");
		expect(deliveryUnit).toContain("auto-edit the body");
		expect(deliveryUnit).toContain("Verification never edits the PR body");
	});

	it("pins complete fail-closed native relationship and PR reads", () => {
		for (const endpoint of [
			"issues/<issue>/comments?per_page=100",
			"issues/<issue>/sub_issues?per_page=100",
			"issues/<issue>/parent",
			"issues/<issue>/dependencies/blocked_by?per_page=100",
		]) {
			expect(deliveryUnit).toContain(endpoint);
		}
		expect(deliveryUnit).toContain("No parent issue found");
		expect(deliveryUnit).toContain("includeClosedPrs: true");
		expect(deliveryUnit).toContain("pageInfo { hasNextPage endCursor }");
		expect(deliveryUnit).toContain("missing or non-advancing `endCursor` holds");
		expect(deliveryUnit).not.toContain("mach12:gh-sub-issues");
	});

	it("pins classification, audit records, plan freshness, and blocker delivery", () => {
		for (const semanticPin of [
			"first nonblank line is exactly `<!-- mach12-initiative-v1 -->`",
			"first nonblank line is exactly `<!-- mach12-batch-v1 -->`",
			"<!-- mach12-membership-decision-v1 -->",
			"Plan-impact: initial-plan-required|replan-required",
			"Approval: user-confirmed",
			"Supersedes: issuecomment-<id>|none",
			"<!-- mach12-disposition-v1 -->",
			"Every later active `Before`, `Destination-before`, and `Dependencies-before`",
			"Final active snapshots",
			"latest exact `<!-- mach12-plan -->` comment",
			"material body or comment requirement added after the plan requires a revised plan",
			"exact retained native member set",
			"exactly one claiming PR in any state",
			"sole claimant is merged",
		]) {
			expect(deliveryUnit).toContain(semanticPin);
		}
	});

	it("pins exact creation and verification safety outcomes", () => {
		for (const semanticPin of [
			"close set is exactly `{D}`",
			"close set is exactly `{D} ∪ exact current direct retained source members}`",
			"`part-of` is exactly that initiative",
			"Partial completion never silently narrows the close set",
			"must have zero claiming PRs in every state",
			"must have claimant set exactly `{current PR}`",
			"report sorted `missing` and `extra` issue numbers",
			"Never close an initiative, sibling, removed source, successor, transitive descendant",
			"Return only one of these verdicts: `ok` or `hold`",
		]) {
			expect(deliveryUnit).toContain(semanticPin);
		}
	});

	it("keeps advisory sub-issue discovery out of destructive linkage", () => {
		const subIssues = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:gh-sub-issues.md`), "utf-8");
		expect(subIssues).toContain("advisory and may fail open through body parsing");
		expect(subIssues).toContain("Never use it for PR close-set derivation or delivery verification");
		expect(subIssues).toContain("must use `mach12:gh-delivery-unit`");
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
