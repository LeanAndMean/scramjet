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
			candidates: [
				{ name: "mach12:pr-review-fix" },
				{ name: "mach12:pr-review" },
				{ name: "mach12:pr-validation" },
				{ name: "mach12:pr-pre-merge" },
			],
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
	{ basename: "plan-comment-contract", expected: null, delegateOnly: true },
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

	it.each(["pr-validation", "pr-validation-assessment"])(
		"%s declares the repository and subagent capabilities needed by executable validation",
		(basename) => {
			const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
			const content = readFileSync(filePath, "utf-8");
			const result = parseCommandFile(filePath, content, SET_NAME);

			expect(result.ok).toBe(true);
			if (!result.ok) return;
			expect(result.def.allowedTools).toEqual([
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

describe("mach12 plan-comment artifact contract", () => {
	const contractPath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:plan-comment-contract.md`);
	const contract = readFileSync(contractPath, "utf-8");
	const issuePlan = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-plan.md`), "utf-8");
	const issueReview = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-review.md`), "utf-8");
	const ghComment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:gh-comment.md`), "utf-8");

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
			"repeated Solution Assessments",
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

	it("orders drafting, validation, approval, unchanged handoff, and posting", () => {
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
			/Display the exact,\s+complete marker-bearing body/,
			/After the user approves the plan/,
			/Pass the exact approved body unchanged/,
			/\/mach12:gh-comment\s+issue\s+<issue-number>/,
		]);
		expectInOrder(issueReview, [
			/\/mach12:plan-comment-contract\s+revision/,
			/Produce the exact,\s+complete standalone replacement/,
			/Assess the finalized candidate/,
			/Only after the Critical\/Important delta gate passes/,
			/After the gate passes, ask the user how to proceed/,
			/When the user picks "Post revised plan"/,
			/pass the exact approved body unchanged/,
			/\/mach12:gh-comment\s+issue\s+<issue-number>/,
		]);
	});

	it("blocks revised-plan publication on unresolved significant deltas", () => {
		expect(issueReview).toContain("treat the candidate as invalid");
		expect(issueReview).toContain("do not offer **Post revised plan**");
		expect(issueReview).toContain("Repeat this gate until no Critical or Important delta remains");
		expect(issueReview).toContain("Suggestions stay visible but are optional and do not block publication");
	});

	it("posts the prepared comment body unchanged through collision-safe stdin", () => {
		expect(ghComment).toContain("Treat the body the caller prepared as immutable");
		expect(ghComment).toContain("does not occur as a standalone line anywhere in the prepared body");
		expect(ghComment).toContain("If it is not newline-terminated or its final-newline state cannot be verified");
		expect(ghComment).toContain("return an error without posting");
		expect(ghComment).toContain("--body-file -");
		expect(ghComment).toContain("<<'MACH12_COMMENT_BODY'");
		expect(ghComment).toContain("Insert the verified body exactly");
		expect(ghComment).not.toContain('--body "$(cat');
	});
});

describe("mach12 issue creation — context provenance", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const body = issueCreate.slice(issueCreate.indexOf("### Body"), issueCreate.indexOf("### Adaptive layouts"));
	const contextContract = body.slice(body.indexOf("**Context**"), body.indexOf("**Investigation**"));
	const acceptanceCriteriaContract = body.slice(
		body.indexOf("**Acceptance Criteria**"),
		body.indexOf("**Open Questions**"),
	);
	const adaptiveLayouts = issueCreate.slice(
		issueCreate.indexOf("### Adaptive layouts"),
		issueCreate.indexOf("### Drafting notes"),
	);
	const finalSelfCheck = issueCreate.slice(
		issueCreate.indexOf("**Final issue-quality self-check before presenting the draft**"),
		issueCreate.indexOf("## Step 3: Review"),
	);

	it("places attributed Context between the user's request and verified investigation", () => {
		expect(body.indexOf("**User's Request**")).toBeLessThan(body.indexOf("**Context**"));
		expect(body.indexOf("**Context**")).toBeLessThan(body.indexOf("**Investigation**"));
		expect(body).toContain("situational background or provenance");
		expect(contextContract).toContain("Identify the source");
		expect(contextContract).toMatch(/attribution[^.]*provenance[^.]*does not make[^.]*verified evidence/i);
		expect(body).toContain("omit this section");
	});

	it("keeps Context distinct from neighboring authority tiers", () => {
		expect(contextContract).toMatch(/must not[^.]*restate Summary/i);
		expect(contextContract).toMatch(/must not[^.]*paraphrase[^.]*User's Request/i);
		expect(contextContract).toMatch(/must not[^.]*verified current-state observations[^.]*Investigation/i);
		expect(contextContract).toMatch(/must not[^.]*reasoning or conclusions[^.]*Analysis/i);
		expect(body).toMatch(/User's Request \+ Context \+ Investigation \+ Analysis/);
	});

	it("does not derive requirements or acceptance criteria from Context alone", () => {
		expect(acceptanceCriteriaContract).toMatch(
			/Context alone must not generate requirements or acceptance criteria/i,
		);
		expect(acceptanceCriteriaContract).toMatch(
			/Open Questions unless the user confirms them or investigation\/analysis supports them/i,
		);
	});

	it("applies Context conditionally without normalizing structured artifacts", () => {
		expect(adaptiveLayouts).toMatch(/Fully specified requests[^\n]*Context/);
		expect(adaptiveLayouts).toContain("independent of whether investigation occurred");
		expect(adaptiveLayouts).toMatch(
			/Structured artifacts[\s\S]*do not (?:add|inject|synthesize)[^\n]*Context heading/i,
		);
	});

	it("checks Context integrity without inventing background", () => {
		expect(finalSelfCheck).toContain("meaningful, attributed, and non-duplicative");
		expect(finalSelfCheck).toMatch(/requirements[^\n]*User's Request/i);
		expect(finalSelfCheck).toMatch(/verified observations[^\n]*Investigation/i);
		expect(finalSelfCheck).toMatch(/conclusions[^\n]*Analysis/i);
		expect(finalSelfCheck).toMatch(/omit[^\n]*Context[^\n]*rather than invent/i);
	});
});

describe("mach12 issue creation — ambiguous duplicate handling", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const ambiguousMatches = issueCreate.slice(
		issueCreate.indexOf("- **Ambiguous matches**"),
		issueCreate.indexOf("## Step 5: Create"),
	);

	it("requires a structured selection with a contextual recommendation", () => {
		expect(ambiguousMatches).toContain('`get_scramjet_user_input` with `type: "select"`');
		expect(ambiguousMatches).toContain("include all four choices");
		expect(ambiguousMatches).toContain(
			"Recommend the choice best supported by the matches and the user's stated intent",
		);
		expect(ambiguousMatches).toContain("no choice is globally preferred");
	});

	it("offers both create outcomes", () => {
		expect(ambiguousMatches).toContain("**Create without mentioning matches**");
		expect(ambiguousMatches).toContain("**Create and mention selected matches**");
		expect(ambiguousMatches).toContain("Add references only to the matches the user explicitly selected");
		expect(ambiguousMatches).toContain("return to Step 3 for explicit approval");
	});

	it("preserves the approved issue when creating without references", () => {
		expect(ambiguousMatches).toContain("approved title and body unchanged");
		expect(ambiguousMatches).toContain("Do not add links, mentions, or notes derived from the duplicate search");
		expect(ambiguousMatches).toContain("do not post comments to any matched issue");
	});

	it("comments on exactly one selected match and skips creation", () => {
		expect(ambiguousMatches).toContain("ask the user to select exactly one of the listed issues");
		expect(ambiguousMatches).toContain("Only after the user explicitly selects the target");
		expect(ambiguousMatches).toMatch(/post the prepared comment only to that issue/i);
		expect(ambiguousMatches).toContain("skip creation");
	});

	it("publishes nothing when skipped", () => {
		expect(ambiguousMatches).toContain("create no issue and post no relationship comment");
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

	it("infers supported branch issue patterns or proceeds unlinked", () => {
		for (const pattern of ["`feature/issue-55-*`", "`fix/issue-55-*`", "`55-some-description`"]) {
			expect(prCreate).toContain(pattern);
		}
		expect(prCreate).toContain("If no issue was supplied and the branch yields no candidate, proceed unlinked");
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

	it("re-resolves changed linkage and pushes without force", () => {
		expect(prCreate).toContain("closing reference was added or changed");
		expect(prCreate).toContain("repeat Step 1's canonical-number validation and `/mach12:gh-issue-read` contract");
		expect(prCreate).toContain("git push -u origin <branch-name>");
		expect(prCreate).not.toMatch(/git push (?:--force|-f)\b/);
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
