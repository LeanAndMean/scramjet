import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseFrontmatter } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
import { parseAutonomyRecommendations } from "../src/autonomy-settings.js";
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
		expected: {
			mode: "open",
			candidates: [{ name: "mach12:pr-review" }, { name: "mach12:pr-validation" }],
		},
	},
	{
		basename: "pr-review",
		expected: { mode: "forced", target: "mach12:pr-review-assessment" },
	},
	{
		basename: "pr-validation",
		expected: { mode: "forced", target: "mach12:pr-validation-assessment" },
	},
	{
		basename: "pr-validation-assessment",
		expected: {
			mode: "closed",
			candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
		},
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
			candidates: [{ name: "mach12:pr-review" }, { name: "mach12:pr-validation" }, { name: "mach12:pr-pre-merge" }],
		},
	},
	{
		basename: "pr-pre-merge",
		expected: {
			mode: "open",
			candidates: [{ name: "mach12:pr-merge" }, { name: "mach12:pr-review" }, { name: "mach12:pr-validation" }],
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
	{ basename: "plan-comment-contract", expected: null, delegateOnly: true },
	{ basename: "find-contribution-guidelines", expected: null, delegateOnly: true },
	{ basename: "gh-issue-read", expected: null, delegateOnly: true },
	{ basename: "gh-pr-read", expected: null, delegateOnly: true },
	{ basename: "gh-sub-issues", expected: null, delegateOnly: true },
	{ basename: "gh-assign", expected: null, delegateOnly: true },
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

	it.each(["pr-validation", "pr-validation-assessment"])(
		"%s declares the repository and subagent capabilities needed by executable validation",
		(basename) => {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const content = readFileSync(filePath, "utf-8");
			const result = parseCommandFile(filePath, content, SET_NAME);

			expect(result.ok).toBe(true);
			if (!result.ok) return;
			expect(result.def.allowedTools).toEqual([
				"add_pr_comment",
				"bash",
				"read",
				"grep",
				"find",
				"edit",
				"write",
				"subagent",
				"delegate",
			]);
		},
	);
});

describe("mach12 PR review fix — proportional architecture contract", () => {
	const prReviewFix = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-fix.md`), "utf-8");
	const step4 = prReviewFix.slice(prReviewFix.indexOf("## Step 4:"), prReviewFix.indexOf("## Step 5:"));

	it("locks one evidence-informed scope before proportional architecture", () => {
		const phases = [
			"1. **Codebase exploration**",
			"2. **Lock scope and requirements**",
			"3. **Proportional architecture analysis**",
			"4. **Implementation**",
		];
		let offset = 0;
		for (const phase of phases) {
			const index = step4.indexOf(phase, offset);
			expect(index, phase).toBeGreaterThan(-1);
			offset = index + phase.length;
		}

		expect(step4).toMatch(/selected findings[^.]*fixed goal[^.]*user explicitly revises/i);
		expect(step4).toMatch(/ask only unresolved scope or requirement questions here/i);
		expect(step4).toMatch(/skip architect ceremony[^.]*trivial/i);
		expect(step4).toMatch(/unresolved non-trivial architecture requires one or more `mach12:code-architect`/i);
		expect(step4).toMatch(/every architect[^.]*same locked scope/i);
		expect(step4).toMatch(/neither reduce nor expand the locked outcomes/i);
		expect(step4).toMatch(/ask separate architecture questions only after synthesis/i);
		expect(step4).toMatch(/parent[^.]*selects or synthesizes[^.]*smallest supported design/i);
		expect(step4).toMatch(/parent owns the final design, repository mutation, and testing/i);
	});
});

describe("mach12 inline forge publication inventory", () => {
	const expected = new Map<string, string[]>([
		["issue-create", ["create_issue", "add_issue_comment"]],
		["issue-plan", ["add_issue_comment"]],
		["issue-review", ["add_issue_comment"]],
		["pr-create", ["create_pr"]],
		["pr-review", ["add_pr_comment"]],
		["pr-review-assessment", ["create_issue", "add_issue_comment", "add_pr_comment"]],
		["pr-validation", ["add_pr_comment"]],
		["pr-validation-assessment", ["add_pr_comment"]],
		["push", ["add_issue_comment", "add_pr_comment"]],
	]);

	it.each([...expected])("%s directly allows and invokes its publication tools", (basename, tools) => {
		const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
		const content = readFileSync(filePath, "utf-8");
		const parsed = parseCommandFile(filePath, content, SET_NAME);
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		for (const tool of tools) {
			expect(parsed.def.allowedTools).toContain(tool);
			expect(content).toContain(`\`${tool}\``);
		}
		expect(content).not.toContain("mach12:gh-comment");
	});

	it("ships the exact behavior-preserving publication policy matrix", () => {
		const defaults = parseAutonomyRecommendations(
			readFileSync(resolve(MACH12_COMMANDS_DIR, "..", "autonomy-defaults.yaml"), "utf-8"),
		);
		expect(defaults.publications).toEqual({
			"mach12:issue-create": { create_issue: "require-approval", add_issue_comment: "auto-approve" },
			"mach12:issue-implement": { add_issue_comment: "auto-approve", add_pr_comment: "auto-approve" },
			"mach12:issue-plan": { add_issue_comment: "require-approval" },
			"mach12:issue-review": { add_issue_comment: "require-approval" },
			"mach12:pr-create": { create_pr: "require-approval" },
			"mach12:pr-pre-merge": { add_issue_comment: "auto-approve", add_pr_comment: "auto-approve" },
			"mach12:pr-review-assessment": {
				create_issue: "auto-approve",
				add_issue_comment: "auto-approve",
				add_pr_comment: "auto-approve",
			},
			"mach12:pr-review-fix": { add_issue_comment: "auto-approve", add_pr_comment: "auto-approve" },
			"mach12:pr-review": { add_pr_comment: "auto-approve" },
			"mach12:pr-validation": { add_pr_comment: "auto-approve" },
			"mach12:pr-validation-assessment": { add_pr_comment: "auto-approve" },
		});
		for (const [command, settings] of Object.entries(defaults.publications ?? {})) {
			expect(command).not.toBe("mach12:push");
			const filePath = join(MACH12_COMMANDS_DIR, `${command}.md`);
			const parsed = parseCommandFile(filePath, readFileSync(filePath, "utf-8"), SET_NAME);
			expect(parsed.ok).toBe(true);
			if (!parsed.ok) continue;
			expect(parsed.def.delegateOnly).not.toBe(true);
			for (const tool of Object.keys(settings)) expect(parsed.def.allowedTools).toContain(tool);
		}
	});

	it("avoids unconditional approval-card claims in command prose", () => {
		const contents = readdirSync(MACH12_COMMANDS_DIR)
			.filter((file) => file.endsWith(".md"))
			.map((file) => readFileSync(join(MACH12_COMMANDS_DIR, file), "utf-8"))
			.join("\n");
		expect(contents).not.toMatch(
			/sole .*approval|UI owns approval|multiline UI|tool-approved|opening publication approval|owns approval/i,
		);
	});

	it.each(["plan-comment-contract", "issue-create", "issue-plan", "issue-review", "pr-create"])(
		"%s keeps approval conditional while requiring guarded verification",
		(basename) => {
			const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
			expect(content).toContain("When effective policy requires approval, the approval card");
			expect(content).toMatch(/Regardless\s+of policy, guarded publication and exact verification apply/);
		},
	);

	it("leaves no raw supported publication command or obsolete delegate", () => {
		const contents = readdirSync(MACH12_COMMANDS_DIR)
			.filter((file) => file.endsWith(".md"))
			.map((file) => readFileSync(join(MACH12_COMMANDS_DIR, file), "utf-8"))
			.join("\n");
		expect(contents).not.toMatch(/gh issue create|gh pr create|gh issue comment|gh pr comment/);
		expect(existsSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:gh-comment.md`))).toBe(false);
	});

	it.each(["issue-implement", "pr-review-fix", "pr-pre-merge"])(
		"%s permits delegated progress publication",
		(basename) => {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const parsed = parseCommandFile(filePath, readFileSync(filePath, "utf-8"), SET_NAME);
			expect(parsed.ok).toBe(true);
			if (!parsed.ok) return;
			expect(parsed.def.allowedTools).toEqual(expect.arrayContaining(["add_issue_comment", "add_pr_comment"]));
		},
	);
});

describe("mach12 issue planning — architecture choice contract", () => {
	const issuePlan = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-plan.md`), "utf-8");
	const step6Start = issuePlan.indexOf("## Step 6:");
	const step6End = issuePlan.indexOf("## Step 7:");
	const step6 = issuePlan.slice(step6Start, step6End);

	it("presents technical-debt differences and requires an accepted approach before proceeding", () => {
		expect(step6Start).toBeGreaterThan(-1);
		expect(step6End).toBeGreaterThan(step6Start);
		for (const disposition of ["introduces", "retains", "reduces", "avoids"]) {
			expect(step6).toContain(disposition);
		}
		expect(step6).toMatch(/none identified/i);
		expect(step6).toMatch(/future (?:maintenance|cost)/i);
		for (const cost of ["maintenance", "migration", "coupling", "testing", "operational"]) {
			expect(step6).toContain(cost);
		}

		for (const existingRequirement of [
			"brief summary of each approach",
			"trade-offs comparison",
			"recommendation with reasoning",
			"concrete implementation differences",
		]) {
			expect(step6).toContain(existingRequirement);
		}

		expect(step6).toMatch(/three options[^.]*narrow Markdown table/i);
		expect(step6).toMatch(
			/columns, in order: \*\*Option\*\*, \*\*Approach\*\*, \*\*Key difference \/ trade-off\*\*, and \*\*Debt delta\*\*/i,
		);
		expect(step6).toMatch(/use \*\*Option\*\* only for the short lens or option name/i);
		expect(step6).toMatch(
			/in \*\*Approach\*\*[^.]*what the architecture builds[^.]*how it works[^.]*requirement or problem it solves/i,
		);
		expect(step6).toMatch(
			/reserve \*\*Key difference \/ trade-off\*\*[^.]*comparative benefits, costs, and sacrifices[^.]*other options/i,
		);
		expect(step6).toMatch(/against the current implementation[^:]*:\s*`\+` means debt introduced/i);
		expect(step6).toMatch(/`-` means existing debt reduced or removed/i);
		expect(step6).toMatch(/signs indicate direction, not whether an option is good or bad/i);
		expect(step6).toMatch(/option does not need to contain both/i);
		expect(step6).toMatch(/use `None identified`[^;]*; never invent debt/i);
		expect(step6).toMatch(/omit retained debt[^.]*immaterial or common/i);
		expect(step6).toMatch(/materially differentiating retained liability[^.]*in words/i);
		expect(step6).toMatch(/common material retained debt outside the compact table/i);
		expect(step6).toMatch(
			/always present[^.]*detailed trade-offs[^.]*implementation differences[^.]*recommendation with reasoning[^.]*common material debt/i,
		);
		expect(step6).toMatch(/place those details outside the compact table[^.]*table cells verbose/i);
		expect(step6).toMatch(/detailed blueprint rationale outside the table/i);

		const perOptionDebt = step6.search(/each lens must also assess the technical debt/i);
		const synthesis = step6.search(/cross-option technical-debt (?:summary|synthesis)/i);
		const choice = step6.search(/ask the user (?:to choose|which approach)/i);
		expect(perOptionDebt).toBeGreaterThan(-1);
		expect(synthesis).toBeGreaterThan(perOptionDebt);
		expect(choice).toBeGreaterThan(synthesis);
		expect(step6).toMatch(/material differences/i);
		expect(step6).toMatch(/common to all options/i);
		expect(step6).toMatch(/every current option[^.]*unsatisfactory/i);
		expect(step6).toMatch(/reject all current (?:approaches|options)[^.]*request revision/i);
		expect(step6).toMatch(/complete updated (?:option )?comparison/i);
		expect(step6).toMatch(/do not proceed to Step 7[^.]*until[^.]*accepts an approach/i);
	});
});

describe("mach12 plan-comment artifact contract", () => {
	const contractPath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:plan-comment-contract.md`);
	const contract = readFileSync(contractPath, "utf-8");
	const issuePlan = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-plan.md`), "utf-8");
	const issueReview = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-review.md`), "utf-8");

	it("loads one mode through the delegated same-context contract", () => {
		expect(contract.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(contract).toContain("<caller-context>\n$ARGUMENTS\n</caller-context>");
		expect(contract).toContain("does not execute an independent formatter");
		expect(contract).toContain("Resume the caller-owned drafting or revision work");
		expect(contract).toContain("Do not call tools, dispatch subagents, ask the user questions, post to GitHub");
		expect(contract).toMatch(/choose\s+an\s+architecture, classify review findings/);
	});

	it("preserves implementation-critical contracts and revision attribution", () => {
		for (const phrase of [
			"invariants and trust-boundary validation",
			"interfaces and data shapes",
			"ownership and correlation rules",
			"event and mutation ordering",
			"persistence and failure semantics",
			"rollback, atomicity, and retry boundaries",
			"expensive call-site inventories",
			"production-realistic test seams",
			"cross-package build ordering and executable or generated-artifact provenance",
			"generated-output drift recovery steps",
			"stage dependencies and ordering constraints",
			"manual checks, with an explicit statement of whether each blocks stage completion",
		]) {
			expect(contract).toContain(phrase);
		}
		expect(contract).toContain("retain the concrete contract rather than reducing it");
		expect(contract).toContain("standalone replacement, never a patch or delta");
		expect(contract).toContain("[user-decided]");
		expect(contract).toContain("[agent-proposed]");
	});

	it("compresses ceremony and requires an evidence-backed final self-check", () => {
		expect(contract).toContain("Remove material that does not help a fresh implementation session");
		for (const phrase of [
			"raw exploration, journal, or probe transcripts",
			"complete rejected blueprints",
			"repeated solution-selection or ladder analysis",
			"generic repository guidance",
			"duplicated requirements or test matrices",
			"speculative LOC estimates",
			"release-preparation work",
			"qualitative compression, not a numeric byte limit",
		]) {
			expect(contract).toContain(phrase);
		}
		for (const defect of [
			"missing implementation-critical contracts",
			"contradictions",
			"duplicated substantive requirements",
			"incorrect or lost decision attribution",
		]) {
			expect(contract).toContain(defect);
		}
		expect(contract).toContain("do not emit a marker-bearing candidate");
	});

	it("orders drafting, validation, concise context, and inline publication", () => {
		const expectInOrder = (content: string, patterns: RegExp[]) => {
			let offset = 0;
			for (const pattern of patterns) {
				const match = content.slice(offset).search(pattern);
				expect(match, pattern.source).toBeGreaterThan(-1);
				offset += match + 1;
			}
		};

		expectInOrder(issuePlan, [
			/\/mach12:plan-comment-contract\s+initial/,
			/Draft the exact,\s+complete post-ready body/,
			/do not display the complete body/i,
			/without repeating the complete plan/,
			/call `add_issue_comment`/i,
			/When effective policy requires approval, the approval card presents the exact payload/,
		]);
		expectInOrder(issueReview, [
			/\/mach12:plan-comment-contract\s+revision/,
			/Produce the exact,\s+complete standalone replacement/,
			/Assess the finalized candidate/,
			/Only after the Critical\/Important delta gate passes/,
			/without repeating the complete replacement plan/,
			/Call `add_issue_comment`/,
			/When effective policy requires approval, the approval card presents the exact payload/,
		]);
	});

	it("blocks revised-plan publication on unresolved significant deltas", () => {
		expect(issueReview).toContain("treat the candidate as invalid");
		expect(issueReview).toContain("do not call `add_issue_comment`");
		expect(issueReview).toContain("Repeat this gate until no Critical or Important delta remains");
		expect(issueReview).toContain("Suggestions stay visible but are optional and do not block publication");
	});

	it("keeps complete payloads in publication tool arguments with policy-conditional approval", () => {
		expect(contract).toContain("pass the complete final candidate only as arguments");
		expect(contract).toContain(
			"When effective policy requires approval, the approval card presents the exact payload",
		);
		expect(contract).toContain("guarded publication and exact verification apply");
		expect(issuePlan).not.toContain("mach12:gh-comment");
		expect(issueReview).not.toContain("mach12:gh-comment");
	});
});

describe("mach12 issue creation — problem capture and direct drafting", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const section = (start: string, end: string) =>
		issueCreate.slice(issueCreate.indexOf(start), issueCreate.indexOf(end));

	it("identifies the motivating problem before any drafting work", () => {
		expect(issueCreate).toMatch(/## Step 1: Identify the problem/);
		for (const phrase of [
			"descriptive content supplied with the command",
			"structured artifact",
			"immediate session context",
			"recent repository observations already established",
		]) {
			expect(issueCreate).toContain(phrase);
		}
		expect(issueCreate).toMatch(/one candidate[^.]*proceed without asking/i);
		expect(issueCreate).toMatch(/one candidate[^.]*plausible but uncertain[^.]*ask for confirmation or correction/i);
		expect(issueCreate).toMatch(/no (?:candidate|supported candidate)[^.]*ask/i);
		expect(issueCreate).toMatch(/multiple (?:distinct|unrelated) candidates[^.]*ask[^.]*which/i);
		expect(issueCreate).toMatch(/do not (?:silently )?combine/i);
	});

	it("distinguishes material factual premises from attributed user evidence", () => {
		const identification = section("## Step 1: Identify the problem", "## Step 2: Classify the anchored problem");
		for (const phrase of [
			"objectively checkable factual premises",
			"experienced symptoms",
			"constraints and non-goals",
			"implementation preferences",
		]) {
			expect(identification).toContain(phrase);
		}
		expect(identification).toMatch(/falsity[^.]*materially change[^.]*problem/i);
		expect(identification).toMatch(/user intent[^.]*attributed evidence/i);
		expect(identification).toMatch(
			/(?=[^.]*attributed evidence)(?=[^.]*objectively checkable factual premises)[^.]*\./i,
		);
		expect(identification).toMatch(/(?=[^.]*preserve the attribution)(?=[^.]*separately verifying)[^.]*\./i);
		expect(identification).toMatch(/implementation preferences[^.]*not[^.]*contradiction/i);
	});

	it("bounds verification and communicates material contradictions before drafting", () => {
		const exploration = section("## Step 4: Explore current behavior", "## Step 5: Clarify the problem");
		for (const phrase of [
			"current-session observations",
			"minimum authoritative evidence",
			"attributed premise",
			"direct conflicting observation and citation",
			"consequence for accurate issue framing",
		]) {
			expect(exploration).toContain(phrase);
		}
		expect(exploration).toMatch(/verify only[^.]*material[^.]*premises/i);
		expect(exploration).toMatch(/(?=[^.]*confirm or correct)(?=[^.]*premise)(?=[^.]*before drafting)[^.]*\./i);
		expect(exploration).toMatch(/lack of corroboration is not a contradiction/i);
		expect(exploration).toMatch(/failure to reproduce[^.]*in one environment[^.]*does not disprove/i);
		expect(exploration).toMatch(/do not silently (?:replace|substitute)/i);
		expect(exploration).toMatch(/stop (?:repository )?inspection once[^.]*recorded accurately/i);
		for (const phrase of [
			"deep code exploration",
			"solution analysis",
			"architecture selection",
			"staged-scope decisions",
			"/mach12:issue-plan",
		]) {
			expect(exploration).toContain(phrase);
		}
	});

	it("requires confirmation or correction for material contradictions while preserving non-blocking cases", () => {
		const clarification = section("## Step 5: Clarify the problem", "## Step 6: Draft the complete issue");
		expect(clarification).toMatch(
			/(?=[^.]*materially contradicts)(?=[^.]*confirm or correct)(?=[^.]*before drafting)[^.]*\./i,
		);
		expect(clarification).toMatch(/non-material uncertainty[^.]*remain[^.]*without[^.]*clarification/i);
		expect(clarification).toMatch(/disputed (?:user )?experience[^.]*remain[^.]*attributed/i);
		expect(clarification).toMatch(/implementation (?:preferences|choices)[^.]*not[^.]*block/i);
	});

	it("keeps clarification focused on describing the problem rather than planning the solution", () => {
		for (const phrase of [
			"actual and expected behavior",
			"observable impact",
			"user-visible outcome",
			"explicit constraints",
		]) {
			expect(issueCreate).toContain(phrase);
		}
		for (const phrase of ["implementation architecture", "internal component boundaries", "staged delivery scope"]) {
			expect(issueCreate).toContain(phrase);
		}
		expect(issueCreate).toMatch(/must not ask[^.]*implementation architecture/i);
		expect(issueCreate).toContain("Preserve explicit user constraints");
		expect(issueCreate).toContain("/mach12:issue-plan");
	});

	it("orders direct drafting, validation, review, duplicate handling, and inline publication", () => {
		const draft = issueCreate.indexOf("## Step 6: Draft the complete issue");
		const validation = issueCreate.indexOf("## Step 7: Validate the draft");
		const review = issueCreate.indexOf("## Step 8: Review the complete draft");
		const duplicates = issueCreate.indexOf("## Step 9: Check for duplicates");
		const publication = issueCreate.indexOf("## Step 10: Publish the issue");
		expect(draft).toBeGreaterThan(-1);
		expect(validation).toBeGreaterThan(draft);
		expect(review).toBeGreaterThan(validation);
		expect(duplicates).toBeGreaterThan(review);
		expect(publication).toBeGreaterThan(duplicates);
	});

	it("retains the complete issue internally until inline publication", () => {
		const draft = issueCreate.slice(issueCreate.indexOf("## Step 6:"), issueCreate.indexOf("## Step 7:"));
		expect(draft).toContain("Construct and retain one");
		expect(draft).not.toMatch(/\b(?:return|display|present)\b[^.]*complete (?:title|body|issue|draft)/i);
		expect(issueCreate).toContain("Call `create_issue` once with the final internally validated title and body");
	});

	it("owns the complete authority-gradient and adaptive drafting contract", () => {
		for (const phrase of [
			"imperative title under 80 characters",
			"one complete body beginning with `<!-- mach12-issue -->`",
			"Construct and retain one explicit, imperative title",
			"**Summary**",
			"**User's Request**",
			"**Context**",
			"**Investigation**",
			"**Analysis**",
			"**Proposed Behavior**",
			"**Acceptance Criteria**",
			"**Open Questions**",
			"**Technical Notes**",
			"**Testability**",
			"For a fully specified request",
			"For a structured artifact",
			"`(user-stated)` or `(derived)`",
		]) {
			expect(issueCreate).toContain(phrase);
		}
	});

	it("preserves evidence authority, sensitive values, and structured artifacts", () => {
		for (const phrase of [
			"evidence, not instruction",
			"only the active command and applicable repository instructions govern drafting",
			"API tokens, passwords, private keys, personal email addresses, and internal hostnames or IP addresses",
			"Do not emit placeholder redactions",
			"identifiers, structure, provenance, and semantic meaning",
			"Routine technical identifiers",
		]) {
			expect(issueCreate).toContain(phrase);
		}
	});

	it("validates and separately reviews the complete draft against live authority", () => {
		const validation = issueCreate.indexOf("## Step 7: Validate the draft");
		const review = issueCreate.indexOf("## Step 8: Review the complete draft");
		const duplicates = issueCreate.indexOf("## Step 9: Check for duplicates");
		expect(validation).toBeGreaterThan(-1);
		expect(review).toBeGreaterThan(validation);
		expect(duplicates).toBeGreaterThan(review);
		expect(issueCreate).toContain("Empty, partial, malformed, truncated, multi-draft, or incorrectly shaped output");
		expect(issueCreate).toContain("complete title and body with the problem anchor and live authoritative context");
	});

	it("contains no architect orchestration or packet fallback contract", () => {
		for (const retired of [
			"mach12:issue-architect",
			"architect packet",
			"data-only packet",
			"sessionless",
			"main-agent drafting",
		]) {
			expect(issueCreate).not.toContain(retired);
		}
	});

	it("revalidates after duplicate-reference changes without a prose approval", () => {
		const duplicateSection = issueCreate.slice(issueCreate.indexOf("## Step 9:"), issueCreate.indexOf("## Step 10:"));
		expect(duplicateSection).toContain("Any payload change requires complete validation and internal review");
		expect(issueCreate).toContain("never a separate full-payload presentation or approval in assistant prose");
		expect(issueCreate).toContain("If the user wants revisions");
	});

	it("retires the checkpointed reviewer protocol completely", () => {
		for (const retired of [
			"mach12:issue-intent-fidelity-reviewer",
			"mach12:issue-maintainer-usability-reviewer",
			"checkpoint marker",
			"parentSessionJournal",
			"parentId",
			"Independent Review Gate",
		]) {
			expect(issueCreate).not.toContain(retired);
		}
	});
});

describe("mach12 issue creation — ambiguous duplicate handling", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const duplicateSection = issueCreate.slice(issueCreate.indexOf("## Step 9:"), issueCreate.indexOf("## Step 10:"));

	it("resolves every duplicate-dependent payload decision before publication", () => {
		expect(duplicateSection).toContain("create unchanged");
		expect(duplicateSection).toContain("Create and mention selected matches");
		expect(duplicateSection).toContain("Comment on one existing issue");
		expect(duplicateSection).toContain("**Skip:** create nothing and post nothing");
		expect(duplicateSection).toContain("explicit target choice");
		expect(duplicateSection).toContain("call `add_issue_comment`");
	});
});

describe("mach12 issue creation — duplicate search and inline publication", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const duplicateCheck = issueCreate.slice(issueCreate.indexOf("## Step 9:"), issueCreate.indexOf("## Step 10:"));
	const publication = issueCreate.slice(issueCreate.indexOf("## Step 10:"), issueCreate.indexOf("## Step 11:"));

	it("fails closed while transporting and classifying duplicate searches", () => {
		expect(duplicateCheck).toContain("Never interpolate the query into shell source");
		expect(duplicateCheck).toContain("duplicate_search_dir=$(mktemp -d) || {");
		expect(duplicateCheck).toContain("<<'MACH12_DUPLICATE_QUERY' || {");
		expect(duplicateCheck).toContain('--search "$(<"$duplicate_search_dir/query")"');
		expect(duplicateCheck).toContain("require its top-level value to be an array");
		expect(duplicateCheck).toContain("/mach12:gh-issue-read <candidate-number>");
		expect(duplicateCheck).toContain("Only a successfully read candidate can be a clear duplicate");
		expect(duplicateCheck).toContain("old age is insufficient proof that it is obsolete");
	});

	it("uses one inline approval after duplicate-dependent payload shaping", () => {
		expect(issueCreate.indexOf("## Step 9:")).toBeLessThan(issueCreate.indexOf("## Step 10:"));
		expect(publication).toContain("Call `create_issue` once");
		expect(publication).toContain("approval card is the first complete-draft presentation");
		expect(publication).toContain("Do not repeat the complete title or body in prose");
		expect(publication).toContain("Do not retry automatically");
		expect(publication).not.toContain("gh issue create");
	});

	it("applies metadata only after verified creation", () => {
		const metadata = issueCreate.slice(issueCreate.indexOf("## Step 11:"), issueCreate.indexOf("## Step 12:"));
		expect(metadata).toContain("Only after `create_issue` returns verified identity");
		expect(metadata).toContain("separate guarded `gh issue edit` operations");
		expect(metadata).toContain("metadata failure is partial success");
		expect(metadata).toContain("do not recreate the issue");
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
		const draft = prCreate.indexOf("## Step 3: Draft and validate the PR payload");
		expect(ambiguity).toBeGreaterThan(-1);
		expect(ambiguity).toBeLessThan(draft);
		expect(prCreate).toContain("could be either an issue number or general context");
		expect(prCreate).toContain("multiple plausible issue candidates");
		expect(prCreate).toContain("select exactly one issue or explicitly decline linkage");
	});

	it("infers supported branch issue patterns or proceeds unlinked", () => {
		for (const pattern of ["`feature/issue-55-*`", "`fix/issue-55-*`", "`55-some-description`"]) {
			expect(prCreate).toContain(pattern);
		}
		expect(prCreate).toContain("If no issue was supplied and the branch yields no candidate, proceed unlinked");
	});

	it("proposes zero or one closer without relationship expansion", () => {
		expect(prCreate).toContain("exactly one standalone `Fixes #N` line");
		expect(prCreate).toContain("Zero closing-keyword lines");
		expect(prCreate).toContain("at most one closer");
		expect(prCreate).not.toContain("mach12:gh-sub-issues");
		expect(prCreate).not.toContain("close-set");
	});

	it("validates linkage before one inline PR approval", () => {
		expect(prCreate).toContain("zero or one closing-keyword occurrence");
		expect(prCreate).toContain("standalone line with exactly one canonical issue target");
		expect(prCreate).toContain("Internally review the complete title/body");
		expect(prCreate).toContain("without separately displaying or approving the complete payload");
		expect(prCreate).toContain("Call `create_pr` once");
		expect(prCreate).toContain("without repeating the complete title/body");
		expect(prCreate).not.toContain("gh pr create");
	});

	it("re-resolves changed linkage and synchronizes the branch without another confirmation", () => {
		expect(prCreate).toContain("If linkage changes");
		expect(prCreate).toContain("repeat Step 1's canonical-number validation and `/mach12:gh-issue-read` contract");
		expect(prCreate).toContain("Invoking this command authorizes ensuring the branch is available on `origin`");
		expect(prCreate).toContain(
			"local `HEAD`, the configured upstream when present, and a fresh `origin` branch head",
		);
		expect(prCreate).toContain("do not push");
		expect(prCreate).toContain("set that tracking relationship without changing remote content");
		expect(prCreate).toContain("when it is absent or points elsewhere");
		expect(prCreate).toContain("remote branch is absent or strictly behind local `HEAD`");
		expect(prCreate).toContain("git push -u origin <branch-name>");
		expect(prCreate).toContain("remote branch is ahead or diverged");
		expect(prCreate).toContain("Reverify that the fresh remote head equals local `HEAD`");
		expect(prCreate).toContain("whether this invocation pushed the branch or found it already synchronized");
		expect(prCreate).toContain("do not ask for another push confirmation");
		expect(prCreate).not.toMatch(/git push (?:--force|-f)\b/);
	});
});

describe("mach12 GitHub reference authoring contracts", () => {
	const commands = (names: string[]) =>
		names.map((name) => [name, readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${name}.md`), "utf-8")] as const);

	it.each(
		commands([
			"plan-comment-contract",
			"issue-create",
			"issue-plan",
			"issue-review",
			"pr-create",
			"pr-review",
			"pr-review-assessment",
			"push",
		]),
	)("%s distinguishes intentional GitHub relationships from local identifiers", (_name, content) => {
		expect(content).toMatch(/same-repository issue or pull-request (?:references|relationships) use\s+`#N`/);
		expect(content).toContain("`owner/repo#N`");
		expect(content).toMatch(
			/artifact-local(?: identifiers| findings, suggestions, and stages)? use stable labels or plain words/i,
		);
		expect(content).toMatch(/(?:never|rather than)\s+bare `#N`/);
	});

	it("applies the issue-review reference policy before decision-comment preparation", () => {
		const issueReview = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-review.md`), "utf-8");
		const policy = issueReview.indexOf("In every decision comment authored here");
		const preparation = issueReview.indexOf('If the user picks "Proceed as-is"');
		expect(policy).toBeGreaterThan(-1);
		expect(preparation).toBeGreaterThan(-1);
		expect(policy).toBeLessThan(preparation);
	});

	it("links deferred-finding relationships without turning finding labels into links", () => {
		const assessment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-assessment.md`), "utf-8");
		expect(assessment).toContain("Related finding from PR #<pr-number> review");
		expect(assessment).toContain("originating same-repository PR as `#<pr-number>`");
		expect(assessment).toContain("matched issues as `#<issue-number>`");
		expect(assessment).toContain("finding retains its F/S identifier");
	});

	it("preserves the single optional PR closer without an obsolete comment delegate", () => {
		const prCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-create.md`), "utf-8");
		expect(prCreate).toContain("exactly one standalone `Fixes #N` line");
		expect(prCreate).toContain("must not add closing keywords");
		expect(existsSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:gh-comment.md`))).toBe(false);
	});
});

describe("mach12 review-assessment publication boundary", () => {
	const assessment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-assessment.md`), "utf-8");

	it("authenticates an explicit review comment against the target PR", () => {
		const context = assessment.slice(assessment.indexOf("## Step 2:"), assessment.indexOf("## Step 3:"));
		expect(context).toContain("match exactly one comment in that target PR's stream");
		expect(context).toContain("`<!-- mach12-review -->`");
		expect(context).toContain("authenticated `gh api user --jq .login` identity");
	});

	it("uses verified publication directly as the queued-mutation and routing gate", () => {
		const publication = assessment.slice(assessment.indexOf("## Step 7:"));
		expect(publication).toContain("Continue only when publication is verified");
		expect(publication).toContain("verified canonical URL");
		expect(publication).toContain("definite pre-dispatch no-write failure leaves no assessment artifact");
		expect(publication).toContain("An ambiguous publication may have created the assessment");
		expect(publication).toContain("block queued mutations and routing pending deliberate reconciliation");
		expect(publication).toContain(
			"Only after `add_pr_comment` returns a verified assessment publication and its numeric comment ID is retained",
		);
		expect(publication).not.toMatch(/re-fetch|trusted-author authentication|trusted author/);
	});

	it.each(["pr-review", "pr-validation", "pr-validation-assessment"])(
		"trusts verified publication without re-authenticating a new comment in %s",
		(command) => {
			const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${command}.md`), "utf-8");
			const publication = content.slice(content.lastIndexOf("State the"));
			expect(publication).toContain("Continue only when publication is verified");
			expect(publication).toContain("verified canonical URL");
			expect(publication).toMatch(/ambiguity.*(?:automatic retry|retry automatically)/s);
			expect(publication).not.toMatch(/re-fetch|trusted author|trusted-author/);
		},
	);
});

describe("mach12 deferred-review issue labels", () => {
	const assessment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-assessment.md`), "utf-8");
	const deferredSection = assessment.slice(assessment.indexOf("## Step 6:"), assessment.indexOf("## Step 7:"));

	it("discovers the exact label once per issue-creation batch and authorizes creation once", () => {
		const lookupCommand = "gh api --paginate --slurp 'repos/{owner}/{repo}/labels?per_page=100'";
		const createCommand = 'gh label create "PR review deferral"';
		expect(assessment).toContain("get_scramjet_user_input");
		expect(deferredSection).toContain("one issue-creation batch");
		expect(deferredSection).toContain("immediately before finalizing the first queued issue");
		expect(deferredSection.split(lookupCommand)).toHaveLength(2);
		expect(deferredSection.split(createCommand)).toHaveLength(2);
		expect(deferredSection).toContain("array of page arrays");
		expect(deferredSection).toContain("every flattened entry is an object with a string `name`");
		expect(deferredSection).toContain("invalid flattened entry makes availability unknown");
		expect(deferredSection).toContain("Do not prompt or create the label");
		expect(deferredSection).toContain("exact, case-sensitive equality");
		expect(deferredSection.split('type: "confirm"')).toHaveLength(2);
		expect(deferredSection).toContain("queue one guarded `gh label create");
		expect(deferredSection).toContain("Option 3 reuses this same batch decision");
		expect(deferredSection).toContain("resolve the label decision in the **Shared issue-creation batch contract**");
		expect(deferredSection).not.toContain("shared batch label decision described in Option 1");
	});

	it("labels only newly created issues after verified inline creation", () => {
		expect(deferredSection).toContain("Never inspect, create, or apply the label to a clear duplicate");
		expect(deferredSection).toContain("collect the intended disposition without mutating issues");
		expect(deferredSection).toContain("If every item is a clear duplicate, skip label resolution");
		expect(deferredSection).toContain("Record the final title/body");
		expect(deferredSection).toContain("without invoking a publication tool");
		expect(deferredSection).toContain(
			"other relevant labels through separate guarded `gh issue edit --add-label` operations",
		);
		expect(deferredSection).toContain('gh issue edit "$confirmed_issue_url" --add-label "PR review deferral"');
		expect(deferredSection.indexOf("Record the final title/body")).toBeLessThan(
			deferredSection.indexOf('gh issue edit "$confirmed_issue_url" --add-label "PR review deferral"'),
		);
		expect(deferredSection).not.toContain("gh issue create");
	});

	it("queues resolved label handling and pauses all mutation on cancellation", () => {
		expect(deferredSection).toContain("defer label creation and application until Step 7");
		expect(deferredSection).toContain("queue issue creation without the label");
		expect(deferredSection).toContain("one batch-level guidance note");
		expect(deferredSection).toContain("Cancellation is the sole unresolved state");
		expect(deferredSection).toContain("Escape pauses the command before any issue mutation");
		expect(deferredSection).toContain("do not repeat the label prompt");
		expect(deferredSection).toContain("only an explicit resumed authorization may create the label");
		expect(deferredSection).toContain("retain the confirmed issue");
		expect(deferredSection).toContain(
			"Identify the failed label lookup and include concise error context in the CLI summary",
		);
		expect(deferredSection).toContain("If the queued label creation later fails, identify it in the CLI summary");
	});

	it("keeps preparation non-mutating and publishes assessment before deferred requests", () => {
		const summary = assessment.slice(assessment.indexOf("## Step 5:"), assessment.indexOf("## Step 6:"));
		const publish = assessment.indexOf("Call `add_pr_comment` with the PR number and complete final assessment");
		const verified = assessment.indexOf("Continue only when publication is verified", publish);
		const mutations = assessment.indexOf("execute the queued deferred-item requests", verified);
		expect(summary).not.toContain("For each finding");
		expect(summary).not.toContain("staged implementation plan");
		expect(deferredSection).toContain("must not create labels, issues, or comments");
		expect(deferredSection).not.toMatch(/\b(?:Call|call) `(?:create_issue|add_issue_comment|add_pr_comment)`/);
		expect(deferredSection).not.toContain("proceed to create the issue");
		expect(deferredSection).not.toContain("post a comment on the existing issue");
		expect(deferredSection).not.toContain("still create the issue");
		expect(deferredSection).not.toMatch(/\*\*Created(?: \(with overlap note\))?\*\*/);
		expect(deferredSection).toContain("/mach12:gh-issue-read <candidate-number>");
		expect(publish).toBeGreaterThan(-1);
		expect(verified).toBeGreaterThan(publish);
		expect(mutations).toBeGreaterThan(verified);
	});

	it("builds the decision comment only from settled deferred outcomes", () => {
		const requests = assessment.indexOf("execute the queued deferred-item requests");
		const outcomes = assessment.indexOf("classify the result exhaustively", requests);
		const decision = assessment.indexOf("fill every line from these actual outcomes", outcomes);
		expect(requests).toBeGreaterThan(-1);
		expect(outcomes).toBeGreaterThan(requests);
		expect(decision).toBeGreaterThan(outcomes);
	});

	it("distinguishes cancellation, definite no-write failure, and ambiguity", () => {
		expect(deferredSection).toContain(
			"cancellation, definite no-write failure, or ambiguity stops that item's remaining operations",
		);
		const execution = assessment.slice(assessment.indexOf("execute the queued deferred-item requests"));
		expect(execution).toContain(
			"Cancelled:** record that no publication occurred, stop all remaining queued mutation",
		);
		expect(execution).toContain("Ambiguity halts the batch");
		expect(execution).toContain("Never retry any settled or ambiguous publication automatically");
	});
});

describe("mach12 publication routing gates", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const issueReview = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-review.md`), "utf-8");
	const assessment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-assessment.md`), "utf-8");

	it("never falls back to issue creation after the existing-issue comment branch", () => {
		const branch = issueCreate.slice(
			issueCreate.indexOf("Comment on one existing issue"),
			issueCreate.indexOf("## Step 10:"),
		);
		expect(branch).toContain("Every outcome skips Steps 10 and 11");
		expect(branch).toContain("Never call `create_issue` as a fallback");
		expect(branch).toContain('status: "incomplete"');
	});

	it("requires verified revised-plan and proceed decisions before implementation routing", () => {
		expect(issueReview).toContain(
			"only this result counts as an updated plan and makes implementation routing eligible",
		);
		expect(issueReview).toContain("Treat this comment as required before implementation routing");
		expect(issueReview).toContain("no Critical or Important finding remains");
		expect(issueReview).toContain("include **both** declared candidates in `next_steps` only after");
	});

	it("requires verified queued work and audit publication before assessment routing", () => {
		expect(assessment).toContain(
			"every requested queued publication is verified, skipped by explicit user choice, or reclassified as genuine",
		);
		expect(assessment).toContain("any required deferred-disposition decision audit is verified");
		expect(assessment).toContain('Set `status: "completed"` and populate `next_steps` only when');
		expect(assessment).toContain("On a resumed user turn, reconcile the exact target without mutation");
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
	])("%s states ordinary readiness before mutation", (name, content) => {
		const readiness = readinessSection(content);
		for (const predicate of ["open", "non-draft", "required review", "conflict", "checks"]) {
			expect(readiness, `${name}: ${predicate}`).toContain(predicate);
		}
		expect(readiness).toContain("one brief reread");
		expect(content.indexOf(name === "pr-merge" ? "gh pr merge" : "gh pr checkout <pr-number>")).toBeGreaterThan(
			content.indexOf("## Step 3:"),
		);
	});

	it("pre-merge routes remediable outcomes to later steps", () => {
		const readiness = readinessSection(preMerge);
		expect(readiness).toContain("A behind branch continues to Step 5");
		expect(readiness).toContain("pending or failing checks continue to Step 9");
		expect(readiness).toContain("`CONFLICTING` or `DIRTY`");
		expect(readiness).toContain("continue through checkout to Step 5");
		expect(readiness).toContain("does not authorize an automatic merge");
	});

	it("pre-merge keeps conflict remediation behind user confirmation", () => {
		const freshness = preMerge.slice(preMerge.indexOf("## Step 5:"), preMerge.indexOf("## Step 6:"));
		const mergeChoice = freshness.indexOf("**Merge**");
		const cancelChoice = freshness.indexOf("**Cancel**");
		const mergeCommand = freshness.indexOf("git merge origin/<default-branch>");
		expect(mergeChoice).toBeGreaterThan(-1);
		expect(cancelChoice).toBeGreaterThan(mergeChoice);
		expect(mergeCommand).toBeGreaterThan(cancelChoice);
		expect(freshness).toContain("resolve them using codebase context");
		expect(freshness).toContain("genuinely ambiguous");
	});

	it("merge requires ordinary GitHub readiness", () => {
		const readiness = readinessSection(merge);
		expect(readiness).toContain("current with the default branch");
		expect(readiness).toContain("passing its required checks");
		expect(readiness).toContain("repositories without required checks may continue");
	});

	it.each([
		["pr-pre-merge", preMerge],
		["pr-merge", merge],
	])("%s bounds indeterminate readiness and offers no bypass", (_name, content) => {
		expect(readinessSection(content)).toContain("one brief reread");
		expect(readinessSection(content)).toContain("report incomplete rather than guessing");
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

	it("pre-merge gates post-fix verification on a confirmed push", () => {
		const ciSection = preMerge.slice(preMerge.indexOf("## Step 9:"), preMerge.indexOf("## Step 10:"));
		const pushGate = ciSection.indexOf("delegation confirms that the commit was pushed successfully");
		const verify = ciSection.indexOf("### 9d. Verify");
		expect(pushGate).toBeGreaterThan(-1);
		expect(pushGate).toBeLessThan(verify);
		expect(ciSection).toContain("Otherwise report the result and stop before CI verification");
		expect(ciSection).toContain("Wait up to 10 minutes for CI on the pushed fix");
	});

	it("pre-merge uses current checklist references", () => {
		expect(preMerge).toContain("Steps 7a-7d");
		expect(preMerge).not.toContain("Steps 6a-6d");
	});

	it("readiness queries omit unused rollup evidence", () => {
		expect(readinessSection(preMerge)).not.toContain("statusCheckRollup");
		expect(readinessSection(merge)).not.toContain("statusCheckRollup");
	});

	it("pre-merge bounds CI waits", () => {
		const ciSection = preMerge.slice(preMerge.indexOf("## Step 9:"), preMerge.indexOf("## Step 10:"));
		expect(ciSection).toContain("poll for at most 10 minutes");
		expect(ciSection).toContain("report which checks remain pending");
		expect(ciSection).toContain("available logs or provider links");
		expect(ciSection).not.toMatch(/gh pr checks[^\n]*--watch/);
	});

	it("documents release publication as outside the four-tool migration", () => {
		for (const tool of ["create_issue", "create_pr", "add_issue_comment", "add_pr_comment"]) {
			expect(merge).toContain(`\`${tool}\``);
		}
		expect(merge).toContain("explicit exception to inline forge publication");
		expect(merge).toContain("until a release-publication tool exists");
	});

	it("merge confirms GitHub state before cleanup or release", () => {
		const mergeSection = merge.slice(merge.indexOf("## Step 3:"), merge.indexOf("## Step 4:"));
		const mergeCommand = mergeSection.indexOf("gh pr merge <pr-number> --delete-branch");
		const confirmation = mergeSection.indexOf("gh pr view <pr-number> --json state,mergeCommit");
		const cleanup = mergeSection.indexOf("git checkout");
		expect(mergeCommand).toBeGreaterThan(-1);
		expect(confirmation).toBeGreaterThan(mergeCommand);
		expect(cleanup).toBeGreaterThan(confirmation);
		expect(mergeSection).toContain("Before cleanup or release work");
	});

	it("pre-merge defines terminal status predicates and requires final readiness", () => {
		const finalSection = preMerge.slice(preMerge.indexOf("## Step 10:"));
		expect(finalSection).toContain('Report `status: "completed"` only');
		expect(finalSection).toContain('Report `status: "blocked"`');
		expect(finalSection).toContain('Report `status: "incomplete"`');
		expect(finalSection).toContain("final authoritative readiness reread");
		expect(finalSection).toContain("conflict-free");
		expect(finalSection).toContain("conflict remediation was declined or remains unresolved");
		expect(finalSection).not.toContain("or confirmed conflicts");
	});

	it("pre-merge completed reporting offers only merge and optional verification routes", () => {
		const finalSection = preMerge.slice(preMerge.indexOf("## Step 10:"));
		const mergeEntry = finalSection.slice(
			finalSection.indexOf("`/mach12:pr-merge <pr-number>`"),
			finalSection.indexOf("`/mach12:pr-review <pr-number>`"),
		);
		const reviewEntry = finalSection.slice(
			finalSection.indexOf("`/mach12:pr-review <pr-number>`"),
			finalSection.indexOf("`/mach12:pr-validation <pr-number>`"),
		);
		const validationEntry = finalSection.slice(
			finalSection.indexOf("`/mach12:pr-validation <pr-number>`"),
			finalSection.indexOf("- Set `recommended_next_step`"),
		);
		expect(finalSection).toContain("exactly three entries");
		expect(mergeEntry).toContain("`fresh_session`: `true`");
		expect(mergeEntry).toContain("non-empty reason explaining that the PR is merge-ready");
		expect(reviewEntry).toContain("`fresh_session`: `true`");
		expect(reviewEntry).toContain("non-empty reason explaining that additional static review is optional");
		expect(validationEntry).toContain("`fresh_session`: `true`");
		expect(validationEntry).toContain("non-empty reason explaining that executable validation is optional");
		expect(finalSection).toContain("`recommended_next_step` to `0`");
		expect(finalSection).toContain("Do not include `mach12:pr-review-fix`");
		expect(finalSection).toContain("omit `next_steps` and `recommended_next_step`");
		expect(finalSection).not.toContain("`/mach12:pr-review-fix <pr-number>`");
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
			const { frontmatter } = parseFrontmatter<Record<string, unknown>>(content);
			expect(frontmatter.name).toBe(name);
			expect(typeof frontmatter.description).toBe("string");
			expect((frontmatter.description as string).trim().length).toBeGreaterThan(0);
		}
	});
});

describe("mach12 test designer contract", () => {
	it("keeps mach12:test-designer structurally read-only", () => {
		const content = readFileSync(join(MACH12_AGENTS_DIR, "mach12:test-designer.md"), "utf-8");
		const tools = content
			.match(/^tools:\s*(.+)$/m)?.[1]
			.split(",")
			.map((tool) => tool.trim());

		expect(tools).toEqual(["read", "grep", "find", "ls"]);
		expect(content).toContain("never create, edit, remove, format, or execute tests");
	});
});
