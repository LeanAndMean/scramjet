import { execFileSync } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseFrontmatter } from "@leanandmean/coding-agent";
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

	it.each([
		[
			"issue-create",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_issue",
				"create_issue",
				"add_issue_comment",
			],
		],
		[
			"issue-plan",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_issue",
				"add_issue_comment",
			],
		],
		[
			"issue-review",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_issue",
				"add_issue_comment",
			],
		],
		[
			"issue-implement",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"edit",
				"write",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_issue",
			],
		],
		["pr-create", ["bash", "read", "grep", "glob", "get_scramjet_user_input", "read_issue", "create_pr"]],
		[
			"pr-review",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"subagent",
				"get_scramjet_user_input",
				"read_pr",
				"read_issue",
				"add_pr_comment",
			],
		],
		[
			"pr-review-assessment",
			[
				"bash",
				"read",
				"grep",
				"glob",
				"subagent",
				"get_scramjet_user_input",
				"read_pr",
				"edit_pr",
				"read_issue",
				"create_issue",
				"add_issue_comment",
				"add_pr_comment",
			],
		],
		[
			"pr-review-fix",
			[
				"bash",
				"read",
				"grep",
				"find",
				"edit",
				"write",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_pr",
				"add_pr_comment",
			],
		],
		[
			"pr-validation",
			[
				"bash",
				"read",
				"grep",
				"find",
				"edit",
				"write",
				"subagent",
				"get_scramjet_user_input",
				"read_pr",
				"read_issue",
				"add_pr_comment",
			],
		],
		[
			"pr-validation-assessment",
			[
				"bash",
				"read",
				"grep",
				"find",
				"edit",
				"write",
				"subagent",
				"delegate",
				"get_scramjet_user_input",
				"read_pr",
				"read_issue",
				"add_pr_comment",
			],
		],
		[
			"pr-pre-merge",
			["bash", "read", "grep", "glob", "edit", "write", "delegate", "get_scramjet_user_input", "read_pr"],
		],
		["pr-merge", ["bash", "read", "grep", "glob", "read_pr", "read_issue"]],
		["push", ["bash", "read", "grep", "read_issue", "read_pr", "add_issue_comment", "add_pr_comment"]],
	] as const)("%s declares its exact forge-aware tool scope", (basename, expectedTools) => {
		const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`);
		const content = readFileSync(filePath, "utf-8");
		const result = parseCommandFile(filePath, content, SET_NAME);

		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.allowedTools).toEqual(expectedTools);
	});

	it("uses first-class forge tools for covered content operations while retaining excluded shell flows", () => {
		const commands = readdirSync(MACH12_COMMANDS_DIR)
			.filter((file) => file.endsWith(".md"))
			.map((file) => readFileSync(join(MACH12_COMMANDS_DIR, file), "utf-8"))
			.join("\n");

		for (const obsolete of ["gh-issue-read", "gh-pr-read", "gh-comment", "gh-sub-issues"]) {
			expect(commands).not.toContain(obsolete);
		}
		for (const covered of [
			"read_issue",
			"create_issue",
			"add_issue_comment",
			"read_pr",
			"edit_pr",
			"create_pr",
			"add_pr_comment",
		]) {
			expect(commands).toContain(covered);
		}
		expect(commands).not.toMatch(/\bgh issue create\b/);
		expect(commands).not.toMatch(/\bgh pr create\b/);
		expect(commands).not.toMatch(/gh pr view[^\n]*(?:title|body|comments|files|commits)/);
		expect(commands).not.toContain("--method PATCH --raw-field body=");

		for (const retained of [
			"gh issue list",
			"gh issue edit",
			"gh pr checks",
			"gh pr merge",
			"gh release create",
			"gh pr view --json number,url",
		]) {
			expect(commands).toContain(retained);
		}
	});

	it.each([
		"issue-create",
		"issue-plan",
		"issue-review",
		"pr-review",
		"pr-review-assessment",
		"pr-validation",
		"pr-validation-assessment",
		"push",
	])("%s refreshes complete parent evidence in an earlier tool round immediately before comments", (basename) => {
		const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
		expect(content).toContain("Immediately before posting");
		expect(content).toContain("in an earlier assistant tool round");
		expect(content).toContain("returned segment window");
		expect(content).toContain("optional `byte_offset`");
		expect(content).toContain('include: ["artifact", "comments"]');
	});

	it.each(["issue-plan", "issue-implement"])(
		"%s assigns only repository-qualified same-repository children",
		(basename) => {
			const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
			expect(content).toContain("`sub_issues`");
			expect(content).toContain("`html_url`");
			expect(content).toContain("case-insensitively");
			expect(content).toContain("same-repository sub-issue numbers");
			expect(content).toContain("Never pass an external child's bare number");
			expect(content).toContain("native canonical");
		},
	);

	it("derives the complete review diff from the authoritative PR base", () => {
		const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review.md`), "utf-8");
		const read = content.indexOf('Use `read_pr` with `include: ["artifact", "comments", "files"]`');
		const diff = content.indexOf("git diff --name-only");
		expect(read).toBeGreaterThan(-1);
		expect(read).toBeLessThan(diff);
		expect(content).toContain("non-`main` targets such as `release/next`");
		expect(content).toContain('git check-ref-format --branch "$base_branch"');
		expect(content).toContain('git fetch origin "refs/heads/$base_branch:refs/remotes/origin/$base_branch"');
		expect(content).toContain("refs/remotes/origin/$base_branch...HEAD");
		expect(content).not.toContain("origin/main...HEAD");
	});

	it("populates a non-main remote-tracking base in a single-branch clone", () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-pr-base-"));
		const remote = join(root, "remote.git");
		const seed = join(root, "seed");
		const clone = join(root, "clone");
		const git = (args: string[], cwd = root) => execFileSync("git", args, { cwd, stdio: "ignore" });
		try {
			git(["init", "--bare", remote]);
			git(["init", seed]);
			git(["config", "user.email", "test@example.com"], seed);
			git(["config", "user.name", "Test"], seed);
			writeFileSync(join(seed, "file.txt"), "main\n");
			git(["add", "file.txt"], seed);
			git(["commit", "-m", "main"], seed);
			git(["branch", "-M", "main"], seed);
			git(["checkout", "-b", "release/next"], seed);
			writeFileSync(join(seed, "file.txt"), "release\n");
			git(["commit", "-am", "release"], seed);
			git(["push", remote, "main", "release/next"], seed);
			git(["clone", "--single-branch", "--branch", "main", remote, clone]);
			expect(() => git(["rev-parse", "--verify", "refs/remotes/origin/release/next"], clone)).toThrow();

			const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review.md`), "utf-8");
			const match = /git fetch origin "([^"]+)"/.exec(content);
			if (match === null) throw new Error("Missing explicit base fetch refspec");
			git(["fetch", "origin", match[1].replaceAll("$base_branch", "release/next")], clone);
			expect(() => git(["rev-parse", "--verify", "refs/remotes/origin/release/next"], clone)).not.toThrow();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it.each(["pr-review-assessment", "pr-review-fix"])(
		"%s selects ordinary review bodies from read_pr rather than a raw comment-content request",
		(basename) => {
			const content = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
			expect(content).not.toContain("gh api repos/:owner/:repo/issues/comments/");
			expect(content).toContain("Do not fetch a second raw copy of content");
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
			/pass the exact approved body unchanged/,
			/add_issue_comment/,
		]);
		expectInOrder(issueReview, [
			/\/mach12:plan-comment-contract\s+revision/,
			/Produce the exact,\s+complete standalone replacement/,
			/Assess the finalized candidate/,
			/Only after the Critical\/Important delta gate passes/,
			/After the gate passes, ask the user how to proceed/,
			/When the user picks "Post revised plan"/,
			/pass the exact approved body unchanged/,
			/add_issue_comment/,
		]);
	});

	it("blocks revised-plan publication on unresolved significant deltas", () => {
		expect(issueReview).toContain("treat the candidate as invalid");
		expect(issueReview).toContain("do not offer **Post revised plan**");
		expect(issueReview).toContain("Repeat this gate until no Critical or Important delta remains");
		expect(issueReview).toContain("Suggestions stay visible but are optional and do not block publication");
	});

	it("passes approved plan bodies unchanged after fresh evidence in an earlier tool round", () => {
		for (const command of [issuePlan, issueReview]) {
			expect(command).toContain("add_issue_comment");
			expect(command).toContain("exact approved body unchanged");
			expect(command).toContain("Immediately before posting");
			expect(command).toContain("in an earlier assistant tool round");
			expect(command).toContain("returned segment window");
			expect(command).toContain("optional `byte_offset`");
		}
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

	it("orders direct drafting, validation, separate review, approval, and duplicate handling", () => {
		const patterns = [
			/Identify the problem/,
			/Classify the anchored problem/,
			/Read project requirements/,
			/Explore current behavior/,
			/Clarify the problem/,
			/Draft the complete issue/,
			/Validate the draft/,
			/Review the complete draft/,
			/Present for approval/,
			/Check for duplicates/,
		];
		let offset = 0;
		for (const pattern of patterns) {
			const match = issueCreate.slice(offset).search(pattern);
			expect(match, pattern.source).toBeGreaterThan(-1);
			offset += match + 1;
		}
	});

	it("owns the complete authority-gradient and adaptive drafting contract", () => {
		for (const phrase of [
			"imperative title under 80 characters",
			"one complete body beginning with `<!-- mach12-issue -->`",
			"no preamble, postscript, alternatives, or commentary-only substitute",
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
		const approval = issueCreate.indexOf("## Step 9: Present for approval");
		expect(validation).toBeGreaterThan(-1);
		expect(review).toBeGreaterThan(validation);
		expect(approval).toBeGreaterThan(review);
		expect(issueCreate).toContain("Empty, partial, malformed, truncated, multi-draft, or incorrectly shaped output");
		expect(issueCreate).toContain("complete title and body with the problem anchor and live authoritative context");
		expect(issueCreate).toContain("repeat complete validation and review");
		expect(issueCreate).toContain("ask only problem-description questions and redraft");

		const finalReview = section("## Step 8: Review the complete draft", "## Step 9: Present for approval");
		expect(finalReview).toMatch(/contradicted or unverified premises[^.]*not[^.]*established facts/i);
		expect(finalReview).toMatch(
			/user intent, experienced symptoms, constraints, and non-goals[^.]*authority and meaning/i,
		);
		expect(finalReview).toMatch(/reconciled disposition[^.]*authority and meaning/i);
	});

	it("contains no architect orchestration or packet fallback contract", () => {
		for (const retired of [
			"mach12:issue-architect",
			"architect packet",
			"data-only packet",
			"sessionless",
			"main-agent drafting",
			"retry automatically",
		]) {
			expect(issueCreate).not.toContain(retired);
		}
	});

	it("revalidates complete drafts after semantic or duplicate-reference changes", () => {
		const approval = issueCreate.slice(
			issueCreate.indexOf("## Step 9: Present for approval"),
			issueCreate.indexOf("## Step 10: Check for duplicates"),
		);
		const semanticModification = approval.indexOf("For a semantic modification");
		const semanticValidation = approval.indexOf("run complete validation", semanticModification);
		const semanticReview = approval.indexOf("followed by complete review", semanticValidation);
		const replacement = approval.indexOf("present the entire reviewed replacement", semanticReview);
		const renewedApproval = approval.indexOf("renewed approval", replacement);
		expect(semanticModification).toBeGreaterThan(-1);
		expect(semanticValidation).toBeGreaterThan(semanticModification);
		expect(semanticReview).toBeGreaterThan(semanticValidation);
		expect(replacement).toBeGreaterThan(semanticReview);
		expect(renewedApproval).toBeGreaterThan(replacement);
		expect(approval).toMatch(/spelling, formatting, labels, or assignees[^.]*no additional content review/i);

		const duplicateReferences = issueCreate.slice(
			issueCreate.indexOf("- **Create and mention selected matches**"),
			issueCreate.indexOf("- **Comment on one existing issue instead**"),
		);
		const duplicateValidation = duplicateReferences.indexOf("run complete validation");
		const duplicateReview = duplicateReferences.indexOf("followed by complete review", duplicateValidation);
		expect(duplicateValidation).toBeGreaterThan(-1);
		expect(duplicateReview).toBeGreaterThan(duplicateValidation);
		expect(duplicateReferences).toContain("complete updated title and body");
		expect(issueCreate).toContain("latest explicitly approved title and body unchanged");
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
	const ambiguousStart = issueCreate.indexOf("- **Ambiguous matches**");
	const createStart = issueCreate.indexOf("## Step 11: Create");
	const ambiguousMatches = issueCreate.slice(ambiguousStart, createStart);

	it("scopes assertions to the duplicate-handling section", () => {
		expect(ambiguousStart).toBeGreaterThan(-1);
		expect(createStart).toBeGreaterThan(ambiguousStart);
	});

	it("requires a structured selection with a contextual recommendation", () => {
		expect(ambiguousMatches).toContain('`get_scramjet_user_input` with `type: "select"`');
		expect(ambiguousMatches).toContain("include all four choices");
		expect(ambiguousMatches).toContain(
			"Recommend the choice best supported by the readable matches and the user's stated intent",
		);
		expect(ambiguousMatches).toContain("no choice is globally preferred");
	});

	it("offers both create outcomes", () => {
		expect(ambiguousMatches).toContain("**Create without mentioning matches**");
		expect(ambiguousMatches).toContain("**Create and mention selected matches**");
		expect(ambiguousMatches).toContain("Add references only to the readable matches the user explicitly selected");
		expect(ambiguousMatches).toContain("present that entire replacement using Step 9's approval choices");
		expect(ambiguousMatches).toContain("After renewed approval, continue directly to Step 11");
		expect(ambiguousMatches).toContain("do not repeat Step 10 or the duplicate search");
	});

	it("preserves the approved issue when creating without references", () => {
		expect(ambiguousMatches).toContain("approved title and body unchanged");
		expect(ambiguousMatches).toContain("Do not add links, mentions, or notes derived from the duplicate search");
		expect(ambiguousMatches).toContain("do not post comments to any matched issue");
	});

	it("comments on exactly one selected match and skips creation", () => {
		expect(ambiguousMatches).toContain("ask the user to select exactly one successfully read issue");
		expect(ambiguousMatches).toContain("Only after the user explicitly selects the target");
		expect(ambiguousMatches).toContain(
			"pass that exact prepared body to `add_issue_comment` for only the chosen issue",
		);
		expect(ambiguousMatches).toContain("skip creation");
	});

	it("publishes nothing when skipped", () => {
		expect(ambiguousMatches).toContain("create no issue and post no relationship comment");
	});
});

describe("mach12 issue creation — duplicate search and publication safety", () => {
	const issueCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-create.md`), "utf-8");
	const duplicateCheck = issueCreate.slice(
		issueCreate.indexOf("## Step 10: Check for duplicates"),
		issueCreate.indexOf("## Step 11: Create"),
	);
	const creation = issueCreate.slice(
		issueCreate.indexOf("## Step 11: Create"),
		issueCreate.indexOf("## Step 12: Confirm"),
	);

	it("fails closed unless duplicate search succeeds with a valid JSON array", () => {
		const guardedSearch = duplicateCheck.indexOf("if ! duplicate_json=$(gh issue list");
		const arrayValidation = duplicateCheck.indexOf("jq -e 'type == \"array\"'");
		const resultHandling = duplicateCheck.indexOf("Handle a successfully parsed array");
		expect(guardedSearch).toBeGreaterThan(-1);
		expect(arrayValidation).toBeGreaterThan(guardedSearch);
		expect(resultHandling).toBeGreaterThan(arrayValidation);
		expect(duplicateCheck).toContain("exit 1");
		expect(duplicateCheck).toContain("Do not interpret stdout unless `gh` exited successfully");
		expect(duplicateCheck).toContain("require its top-level value to be an array");
		expect(duplicateCheck).toMatch(/execution fails or parsing or shape validation fails[^.]*stop before Step 11/i);
		expect(duplicateCheck).toContain("parsed array has length zero");
	});

	it("transports the duplicate query without shell interpolation", () => {
		const queryWrite = duplicateCheck.indexOf('cat >"$duplicate_search_dir/query"');
		const guardedSearch = duplicateCheck.indexOf("if ! duplicate_json=$(gh issue list");
		expect(queryWrite).toBeGreaterThan(-1);
		expect(guardedSearch).toBeGreaterThan(queryWrite);
		expect(duplicateCheck).toContain("Never interpolate the query into shell source");
		expect(duplicateCheck).toContain("does not occur as a standalone line in the query");
		expect(duplicateCheck).toContain("duplicate_search_dir=$(mktemp -d) || {");
		expect(duplicateCheck).toContain("Could not create duplicate-search transport directory");
		expect(duplicateCheck).toContain("<<'MACH12_DUPLICATE_QUERY' || {");
		expect(duplicateCheck).toContain("Could not write duplicate-search query");
		expect(duplicateCheck).toContain('--search "$(<"$duplicate_search_dir/query")"');
		expect(duplicateCheck).not.toContain('--search "<keywords>"');
	});

	it("passes the exact approved title and body to create_issue", () => {
		expect(creation).toContain("create_issue");
		expect(creation).toContain("latest explicitly approved title and body unchanged");
		expect(creation).toContain("approved title is one line");
		expect(creation).toContain("exact approved body may end with or without a newline");
		expect(creation).not.toContain("approved body is newline-terminated");
		expect(creation).toContain("exact Markdown body");
		expect(creation).not.toContain("issue_transport_dir");
		expect(creation).not.toContain("gh issue create");
	});

	it("requires verified creation before separate metadata mutations", () => {
		const create = creation.indexOf("create_issue");
		const identity = creation.indexOf("verified canonical URL");
		const metadata = creation.indexOf(
			"Apply each user-requested or repository-standard label and assignee operation",
		);
		expect(create).toBeGreaterThan(-1);
		expect(identity).toBeGreaterThan(create);
		expect(metadata).toBeGreaterThan(identity);
		expect(creation).toContain("Derive the issue number from that URL's validated artifact path");
		expect(creation).toContain("does not return the number separately in model-visible content");
		expect(creation).toContain("do not retry creation");
		expect(creation).toContain("may have succeeded");
		expect(creation).toContain("requiring the command to succeed and return exactly one non-empty login");
		expect(creation).toMatch(/resolution fails[^.]*verified issue URL and derived number[^.]*non-completed status/i);
		expect(creation).toMatch(
			/metadata operation fails[^.]*verified issue URL and derived number[^.]*exact label or assignee operation/i,
		);
		expect(creation).toMatch(/do not retry issue creation or claim complete success[^.]*non-completed status/i);
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
		expect(prCreate.indexOf("Immediately before creation")).toBeLessThan(prCreate.indexOf("Call `create_pr`"));
		expect(prCreate).toContain("Present the validated title and complete body");
		expect(prCreate).toContain("Approve, Modify, or Cancel");
		expect(prCreate).toContain("exact approved title and body");
		expect(prCreate).toContain("returns only the verified canonical PR URL in model-visible content");
		expect(prCreate).toContain("Derive the PR number from that URL's validated artifact path");
		expect(prCreate).toContain('Report `status: "incomplete"` if the user cancelled');
	});

	it("re-resolves changed linkage and pushes without force", () => {
		expect(prCreate).toContain("closing reference was added or changed");
		expect(prCreate).toContain("repeat Step 1's canonical-number validation and complete `read_issue` contract");
		expect(prCreate).toContain("git push -u origin <branch-name>");
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

	it("preserves the single optional PR closer while using verified content tools", () => {
		const prCreate = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-create.md`), "utf-8");
		expect(prCreate).toContain("exactly one optional standalone `Fixes #N` line");
		expect(prCreate).toContain("Ordinary references must not add closing keywords");
		expect(prCreate).toContain("create_pr");
		expect(prCreate).not.toContain("gh pr create");
	});
});

describe("mach12 deferred-review issue labels", () => {
	const assessment = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-review-assessment.md`), "utf-8");
	const deferredSection = assessment.slice(assessment.indexOf("## Step 6:"), assessment.indexOf("## Step 7:"));

	it("requires approval of each exact finalized issue immediately before publication", () => {
		const publication = deferredSection.slice(
			deferredSection.indexOf("For each item that requires a new issue:"),
			deferredSection.indexOf("When lookup, authorization, or label creation"),
		);
		const finalize = publication.indexOf("Finalize its exact title and body");
		const present = publication.indexOf("Show the user that exact final title and complete body");
		const approve = publication.indexOf('`type: "confirm"`', present);
		const create = publication.indexOf("Create the issue with `create_issue`", approve);
		expect(finalize).toBeGreaterThan(-1);
		expect(present).toBeGreaterThan(finalize);
		expect(approve).toBeGreaterThan(present);
		expect(create).toBeGreaterThan(approve);
		expect(publication).toContain("including every overlap note");
		expect(publication).toContain("immediately before publication");
		expect(publication.slice(approve, create)).not.toContain("`gh ");
		expect(publication).toContain("using the approved title and body unchanged");
		expect(publication).toContain("batch-level choice to create issues is not exact-content approval");
	});

	it("discovers the exact label once per issue-creation batch and authorizes creation once", () => {
		const lookupCommand = "gh api --paginate --slurp 'repos/{owner}/{repo}/labels?per_page=100'";
		const createCommand = 'gh label create "PR review deferral"';
		expect(assessment).toContain("get_scramjet_user_input");
		expect(deferredSection).toContain("one issue-creation batch");
		expect(deferredSection).toContain("immediately before the first item that will actually create an issue");
		expect(deferredSection.split(lookupCommand)).toHaveLength(2);
		expect(deferredSection.split(createCommand)).toHaveLength(2);
		expect(deferredSection).toContain("array of page arrays");
		expect(deferredSection).toContain("every flattened entry is an object with a string `name`");
		expect(deferredSection).toContain("invalid flattened entry makes availability unknown");
		expect(deferredSection).toContain("Do not prompt or create the label");
		expect(deferredSection).toContain("exact, case-sensitive equality");
		expect(deferredSection.split('type: "confirm"')).toHaveLength(3);
		expect(deferredSection).toContain("at most one label-creation attempt");
		expect(deferredSection).toContain("Option 3 reuses this same batch decision");
		expect(deferredSection).toContain("resolve the label decision in the **Shared issue-creation batch contract**");
		expect(deferredSection).not.toContain("shared batch label decision described in Option 1");
	});

	it("labels only newly created issues after verified tool identity", () => {
		expect(deferredSection).toContain("Never inspect, create, or apply the label to a clear duplicate");
		expect(deferredSection).toContain("collect the intended disposition without mutating issues yet");
		expect(deferredSection).toContain("If every item is a clear duplicate, skip label resolution");
		expect(deferredSection).toContain(
			"Create the issue with `create_issue` using the approved title and body unchanged, without coupling publication to labels",
		);
		expect(deferredSection).toContain(
			"other relevant labels through separate guarded `gh issue edit --add-label` operations",
		);
		expect(deferredSection).toContain('gh issue edit "$verified_issue_url" --add-label "PR review deferral"');
		expect(deferredSection.indexOf("create_issue")).toBeLessThan(
			deferredSection.indexOf('gh issue edit "$verified_issue_url" --add-label "PR review deferral"'),
		);
		expect(deferredSection).toContain("verified canonical issue URL");
		expect(deferredSection).toContain("Derive the issue number from that URL's validated artifact path");
		expect(deferredSection).toContain("does not return the number separately in model-visible content");
		expect(deferredSection).not.toContain("gh issue create");
		expect(deferredSection).not.toContain("gh issue view");
	});

	it("allows mutation after every resolved label decision and pauses only on cancellation", () => {
		for (const outcome of [
			"exact label is found or created",
			"explicitly declines creation",
			"label lookup fails or is malformed",
			"label creation fails",
		]) {
			expect(deferredSection).toContain(outcome);
		}
		expect(deferredSection).toContain("continue creating issues without the label");
		expect(deferredSection).toContain("one batch-level guidance note");
		expect(deferredSection).toContain("Cancellation is the sole unresolved state");
		expect(deferredSection).toContain("Escape pauses the command before any issue mutation");
		expect(deferredSection).toContain("do not repeat the label prompt");
		expect(deferredSection).toContain("only an explicit resumed authorization may create the label");
		expect(deferredSection).toContain("retain the confirmed issue");
		expect(deferredSection).toContain(
			"Identify the failed label lookup and include concise error context in the CLI summary",
		);
		expect(deferredSection).toContain(
			"If label creation fails, identify the failed label creation and include concise error context in the CLI summary",
		);
	});

	it("preserves definite and ambiguous creation failure semantics", () => {
		expect(deferredSection).toContain("If `create_issue` fails, surface the error");
		expect(deferredSection).toContain("apply no metadata");
		expect(deferredSection).toContain("do not retry automatically");
		expect(deferredSection).toContain("may have succeeded");
		expect(deferredSection).toContain("return a non-completed command status");
	});
});

describe("mach12 issue implementation context", () => {
	const issueImplement = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:issue-implement.md`), "utf-8");

	it("completes the issue read before deriving a branch name", () => {
		const completeInstruction =
			"continue every returned segment window with its returned `include`, `offset`, optional `byte_offset`, and unchanged snapshot";
		const completeRead = issueImplement.indexOf(completeInstruction);
		const branchDerivation = issueImplement.indexOf(
			"Use the issue title from the retained complete `read_issue` result",
		);
		expect(completeRead).toBeGreaterThan(-1);
		expect(branchDerivation).toBeGreaterThan(completeRead);
		expect(issueImplement).not.toContain("artifact title is in the initial XML range");
		expect(issueImplement).toContain("Retain those complete replies");
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
	])("%s requires complete segment-scoped conversation continuation", (_name, content) => {
		const readiness = readinessSection(content);
		expect(readiness).toContain('include: ["artifact", "comments"]');
		expect(readiness).toContain("returned segment window");
		expect(readiness).toContain("optional `byte_offset`");
	});

	it.each([
		["pr-pre-merge", preMerge],
		["pr-merge", merge],
	])("%s states ordinary readiness before mutation", (name, content) => {
		const readiness = readinessSection(content);
		for (const predicate of ["open", "non draft", "required review", "conflict", "checks"]) {
			expect(readiness.replaceAll("-", " "), `${name}: ${predicate}`).toContain(predicate);
		}
		expect(readiness).toContain("provider-native object facts");
		expect(readiness).toContain("narrow provider query");
		expect(readiness).toContain("otherwise report incomplete rather than treating an absent field as review-clear");
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

	it("keeps the feature-completeness checker read-only with first-class forge reads", () => {
		const content = readFileSync(join(MACH12_AGENTS_DIR, "mach12:feature-completeness-checker.md"), "utf-8");
		const tools = content
			.match(/^tools:\s*(.+)$/m)?.[1]
			.split(",")
			.map((tool) => tool.trim());

		expect(tools).toEqual(["read", "grep", "find", "ls", "read_issue", "read_pr"]);
		expect(tools).not.toContain("bash");
		expect(content).toContain("Use the complete diff supplied by the parent review command");
	});

	it("requires snapshot continuation for feature-completeness forge reads", () => {
		const content = readFileSync(join(MACH12_AGENTS_DIR, "mach12:feature-completeness-checker.md"), "utf-8");
		const contextStep = content.slice(
			content.indexOf("### Step 1: Gather Requirements Context"),
			content.indexOf("### Step 2: Catalog the Actual Changes"),
		);

		for (const tool of ["read_pr", "read_issue"]) {
			const instruction = contextStep.split("\n").find((line) => line.includes(`Use \`${tool}\``));
			expect(instruction).toContain("continue every returned segment window");
			expect(instruction).toContain("optional `byte_offset`");
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
