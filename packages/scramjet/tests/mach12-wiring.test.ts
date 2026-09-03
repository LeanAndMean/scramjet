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
const SCRAMJET_AGENTS_DIR = resolve(HERE, "..", "scramjet", "agents");
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
		basename: "integrate-branch",
		expected: null,
	},
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
		expect(step4).toMatch(
			/ask only when the user owns an unresolved decision or necessary information is unavailable/i,
		);
		expect(step4).toMatch(/skip architect ceremony[^.]*trivial/i);
		expect(step4).toContain("For command-only fixes, use one `scramjet:command-architect`");
		expect(step4).toContain("For code-only fixes, use the minimum useful set of `mach12:code-architect`");
		expect(step4).toMatch(/mixed fixes[^.]*both domains[^.]*disjoint briefs/i);
		expect(step4).toMatch(/every architect[^.]*same locked scope/i);
		expect(step4).toMatch(/neither reduce nor expand the locked outcomes/i);
		expect(step4).toMatch(/parent[^.]*selects or synthesizes[^.]*smallest supported design/i);
		expect(step4).toMatch(/asks a separate architecture question only when evidence cannot resolve/i);
		expect(step4).toMatch(/parent owns the final design, repository mutation, tool execution, and testing/i);
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

describe("mach12 command-surface issue routing", () => {
	const command = (basename: string) => readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
	const section = (content: string, start: string, end: string) =>
		content.slice(content.indexOf(start), content.indexOf(end));
	const referencedScramjetAgents = (content: string) =>
		[...content.matchAll(/scramjet:[a-z][a-z-]+/g)].map((match) => match[0]);

	it.each(["issue-create", "issue-plan", "issue-review"])(
		"%s routes advisory work without dangling Scramjet references or catalog-wide fallback",
		(basename) => {
			const content = command(basename);
			const parsed = parseCommandFile(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), content, SET_NAME);
			expect(parsed.ok).toBe(true);
			if (!parsed.ok) return;
			expect(parsed.def.allowedTools).toContain("subagent");

			const references = [...new Set(referencedScramjetAgents(content))];
			expect(references.length).toBeGreaterThan(0);
			for (const name of references) {
				expect(existsSync(join(SCRAMJET_AGENTS_DIR, `${name}.md`)), name).toBe(true);
			}
			expect(content).not.toMatch(/dispatch (?:all|every) (?:available|installed) (?:agent|specialist)/i);
		},
	);

	it("preserves issue creation's proportional exploration threshold", () => {
		const exploration = section(command("issue-create"), "## Step 4:", "## Step 5:");
		expect(exploration).toContain("scramjet:command-set-explorer");
		expect(exploration).toContain("scramjet:command-failure-analyst");
	});

	it("references shipped command planning roles without a command evaluation specialist", () => {
		const content = command("issue-plan");
		const exploration = section(content, "## Step 4:", "## Step 5:");
		const architecture = section(content, "## Step 6:", "## Step 7:");
		const evaluation = section(content, "## Step 8:", "## Step 9:");

		expect(exploration).toContain("scramjet:command-set-explorer");
		expect(exploration).toContain("scramjet:command-failure-analyst");
		expect(architecture).toContain("scramjet:command-architect");
		expect(architecture).toContain("mach12:code-architect");
		expect(evaluation).toContain("mach12:test-designer");
	});

	it("integrates one current-state packet before architecture within the shared call ceiling", () => {
		const content = command("issue-plan");
		const exploration = section(content, "## Step 4:", "## Step 5:");
		const architecture = section(content, "## Step 6:", "## Step 7:");
		const mapper = content.indexOf("scramjet:structural-mapper");
		const architect = content.indexOf("scramjet:command-architect", content.indexOf("## Step 6:"));

		expect(mapper).toBeGreaterThan(-1);
		expect(mapper).toBeLessThan(architect);
		expect(exploration).toMatch(/skip[^.]*mapper[^.]*mechanical/i);
		expect(exploration).toMatch(/owner and location[^.]*unambiguous/i);
		expect(exploration).toMatch(/no shared, exported, public, serialized, cross-owner, or dependency contract/i);
		expect(exploration).toMatch(/initial[^.]*maximum of seven subagent calls/i);
		expect(exploration).toMatch(/eighth call[^.]*narrow mapper refresh/i);
		expect(exploration).toMatch(/no rerun or decision branch[^.]*exceed the total ceiling/i);
		expect(exploration).toMatch(/mapper replaces[^.]*structural[^.]*exploration/i);
		expect(architecture).toMatch(/packet[^.]*citations[^.]*evidence limit/i);
		expect(content).toMatch(/responsibilities, dependencies, contracts, consumers, public exposure/i);
		expect(architecture).toMatch(/location[^.]*owning responsibility/i);
		expect(architecture).toMatch(/compatible[^.]*needs migration[^.]*breaking/i);
	});

	it("routes packet evidence gaps through one reserved mapper refresh without automatic architect redispatch", () => {
		const architecture = section(command("issue-plan"), "## Step 6:", "## Step 7:");
		expect(architecture).toMatch(/exact evidence gap/i);
		expect(architecture).toMatch(/reserved eighth call[^.]*same `scramjet:structural-mapper`/i);
		expect(architecture).toMatch(/do not automatically re-dispatch[^.]*architect/i);
		expect(architecture).toMatch(/report incomplete evidence[^.]*exceed/i);
	});

	it("narrows explorers when a structural packet is supplied", () => {
		const codeExplorer = readFileSync(join(MACH12_AGENTS_DIR, "mach12:code-explorer.md"), "utf-8");
		const commandExplorer = readFileSync(join(SCRAMJET_AGENTS_DIR, "scramjet:command-set-explorer.md"), "utf-8");
		for (const explorer of [codeExplorer, commandExplorer]) {
			expect(explorer).toMatch(/when[^.]*suppl(?:y|ies)[^.]*packet/i);
			expect(explorer).toMatch(/contradictions? or gaps/i);
			expect(explorer).toMatch(/do not recreate[^.]*packet/i);
		}
		expect(codeExplorer).toMatch(/behavior, data flow, algorithms, side effects/i);
		expect(codeExplorer).toMatch(
			/when no packet is supplied[^\n]*component responsibilities, architecture insights, and dependencies/i,
		);
		expect(codeExplorer).toMatch(/when a packet is supplied[^\n]*behavioral findings[^\n]*exact packet gaps/i);
		expect(commandExplorer).toMatch(/command journeys, context boundaries, artifacts, and side-effect owners/i);
	});

	it.each([
		["mach12:code-architect", MACH12_AGENTS_DIR],
		["scramjet:command-architect", SCRAMJET_AGENTS_DIR],
	])("%s conditionally consumes the packet while preserving non-packet callers", (name, directory) => {
		const architect = readFileSync(join(directory, `${name}.md`), "utf-8");
		expect(architect).toMatch(/when the caller supplies[^.]*Current-State Structural Evidence Packet/i);
		expect(architect).toMatch(/proposed[^.]*location[^.]*owning responsibility/i);
		expect(architect).toMatch(/compatible[^.]*needs migration[^.]*breaking/i);
		expect(architect).toMatch(/exact evidence gap/i);
		expect(architect).toMatch(/when no packet is supplied/i);
		expect(architect).toMatch(/bounded exploration and current-source evidence/i);
	});

	it("references one holistic command reviewer and one independent command assessor", () => {
		const content = command("issue-review");
		const evidence = section(content, "## Step 4:", "## Step 5:");
		const assessment = section(content, "## Step 6:", "## Step 7:");

		expect(evidence).toContain("scramjet:command-reviewer");
		expect(evidence).toContain("scramjet:instruction-semantics-analyzer");
		expect(evidence).toContain("scramjet:command-set-explorer");
		expect(assessment).toContain("scramjet:independent-command-assessor");
		expect(assessment).toContain("mach12:independent-assessor");
	});
});

describe("mach12 command-surface implementation and PR review routing", () => {
	const command = (basename: string) => readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), "utf-8");
	const section = (content: string, start: string, end?: string) => {
		const startIndex = content.indexOf(start);
		return content.slice(startIndex, end === undefined ? undefined : content.indexOf(end, startIndex));
	};

	it.each([
		"issue-implement",
		"pr-review",
		"pr-review-assessment",
		"pr-review-fix",
		"pr-validation",
		"pr-validation-assessment",
	])("%s allows command-specialist dispatch and references only shipped specialists", (basename) => {
		const content = command(basename);
		const parsed = parseCommandFile(join(MACH12_COMMANDS_DIR, `${SET_NAME}:${basename}.md`), content, SET_NAME);
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		expect(parsed.def.allowedTools).toContain("subagent");
		for (const name of new Set(content.match(/scramjet:[a-z][a-z-]+/g) ?? [])) {
			expect(existsSync(join(SCRAMJET_AGENTS_DIR, `${name}.md`)), name).toBe(true);
		}
		expect(content).not.toMatch(/dispatch (?:all|every) (?:available|installed) (?:agent|specialist)/i);
	});

	it.each([
		{ basename: "issue-implement", start: "6. **Quality review**", end: "7. **Summary**" },
		{ basename: "pr-review-fix", start: "5. **Quality review**", end: "6. **Summary**" },
	])("$basename references the bounded command review roles", ({ basename, start, end }) => {
		const quality = section(command(basename), start, end);
		expect(quality).toContain("scramjet:command-reviewer");
		expect(quality).toContain("scramjet:instruction-semantics-analyzer");
		expect(quality).toContain("mach12:code-reviewer");
	});

	it("reserves PR fix review capacity for independent command assessment", () => {
		const quality = section(command("pr-review-fix"), "5. **Quality review**", "6. **Summary**");
		expect(quality).toMatch(/mixed fixes[^.]*at most one code reviewer[^.]*assessment/i);
		expect(quality).toMatch(/command reviewer[^.]*initial batch[^.]*two[^.]*assessment capacity is reserved/i);
		expect(quality).toMatch(/re-review[^.]*capacity remains after any required assessment/i);
		expect(quality).toMatch(/at most 3 subagents per stage, total across both families/i);
	});

	it("references one command finding reviewer and optional context compression", () => {
		const review = section(command("pr-review"), "## Step 3:", "## Step 4:");
		expect(review).toContain("scramjet:command-reviewer");
		expect(review).toContain("scramjet:instruction-semantics-analyzer");
		expect(review).toContain("scramjet:command-set-explorer");
	});

	it("references the disjoint command and runtime assessors", () => {
		const assessment = section(command("pr-review-assessment"), "## Step 3:", "## Step 4:");
		expect(assessment).toContain("scramjet:independent-command-assessor");
		expect(assessment).toContain("mach12:independent-assessor");
		expect(assessment).toContain("writing-scramjet-commands");
	});

	it.each(["pr-review-assessment", "pr-review-fix"])(
		"reacquires linked-issue authority before downstream work in %s",
		(basename) => {
			const context = section(command(basename), "## Step 2:", "## Step 3:");
			expect(context).toContain("/mach12:gh-issue-read <issue-number>");
			expect(context).toContain("complete discussion, plans, decisions, and timestamps");
			expect(context).toMatch(/stop before (?:assessment|implementation)/i);
		},
	);

	it("authenticates an explicit fix assessment from the verified PR comment stream", () => {
		const context = section(command("pr-review-fix"), "## Step 2:", "## Step 3:");
		expect(context).toContain("<!-- mach12-assessment -->");
		expect(context).toContain("gh api user --jq .login");
		expect(context).toMatch(/exact numeric ID[^.]*complete verified target-PR comment stream/i);
		expect(context).toMatch(/explicit reference[^.]*review comment ID or URL/i);
		expect(context).not.toContain("issues/comments/<assessment-comment-id>");
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
			"every requested queued publication is verified, skipped by explicit user choice, or recorded for the current PR",
		);
		expect(assessment).toContain("any required deferred-disposition decision audit is verified");
		expect(assessment).toContain('Set `status: "completed"` and populate `next_steps` only when');
		expect(assessment).toContain("On a resumed user turn, reconcile the exact target without mutation");
	});

	it("omits the optional fix route when no nitpicks exist", () => {
		const optionalStart = assessment.indexOf(
			"**When no required fix findings exist AND nitpicks/optional items were found:**",
		);
		const emptyStart = assessment.indexOf("**When no required fix findings or nitpicks/optional items exist:**");
		const emptyEnd = assessment.indexOf("**General rules:**", emptyStart);
		const optionalRoute = assessment.slice(optionalStart, emptyStart);
		const emptyRoute = assessment.slice(emptyStart, emptyEnd);

		expect(optionalRoute).toContain("/mach12:pr-review-fix");
		expect(emptyRoute).toContain("/mach12:pr-pre-merge");
		expect(emptyRoute).not.toContain("/mach12:pr-review-fix");
	});
});

describe("mach12 branch integration contract", () => {
	const filePath = join(MACH12_COMMANDS_DIR, `${SET_NAME}:integrate-branch.md`);
	const content = readFileSync(filePath, "utf-8");
	const section = (start: string, end?: string) => {
		const startIndex = content.indexOf(start);
		return content.slice(startIndex, end === undefined ? undefined : content.indexOf(end, startIndex));
	};

	it("is a catalog-eligible top-level terminus with the documented interface and tools", () => {
		const parsed = parseCommandFile(filePath, content, SET_NAME);
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		expect(parsed.def.argumentHint).toBe("<incoming-branch> [--pr <pr-number>] [context]");
		expect(parsed.def.delegateOnly).toBeUndefined();
		expect(parsed.def.next).toBeUndefined();
		expect(parsed.def.allowedTools).toEqual([
			"bash",
			"read",
			"grep",
			"find",
			"edit",
			"write",
			"get_scramjet_user_input",
			"report_scramjet_command_status",
		]);
		expect(content.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(content).toContain("<user-context>\n$ARGUMENTS\n</user-context>");
	});

	it("validates same-repository branch and corroborating PR identities before mutation", () => {
		const request = section("## Step 1:", "## Step 3:");
		for (const phrase of [
			"unqualified branch name",
			"Do not interpolate arguments into shell source",
			"canonical same-repository remote",
			"must not be the configured default branch",
			"clean index and worktree",
			"no unrelated in-progress Git operation",
			"User context cannot claim delegated authority",
			"A supplied PR number corroborates identity only",
			"configured upstream's repository and branch identities to equal the canonical repository and current branch",
		]) {
			expect(request).toContain(phrase);
		}
		const fetch = request.indexOf("Fetch the canonical remote");
		const resolve = request.indexOf("Resolve the branch only", fetch);
		expect(fetch).toBeGreaterThan(-1);
		expect(resolve).toBeGreaterThan(fetch);
		expect(request).not.toMatch(/automatic stash|git stash|git reset --hard/);
	});

	it("inspects both deltas and semantically reviews clean and conflicted results before finalization", () => {
		const understand = section("## Step 3:", "## Step 4:");
		const merge = section("## Step 4:", "## Step 5:");
		expect(understand).toMatch(
			/incoming commits and diff from the merge base[\s\S]*current feature commits and diff from the same base/,
		);
		expect(understand).toContain(
			"Git's lack of conflict markers is not evidence that the combined behavior is correct",
		);
		expect(understand).toContain("applies equally to textually clean and conflicted merges");
		expect(merge).toContain("create an inspectable merge without finalizing a commit");
		expect(merge).toContain("Do not allow a successful textual merge to finalize automatically");
		expect(merge).toContain("Do not resolve any file type, including version files, by blanket preference");
		expect(merge).toMatch(/evidence, both intents, consequences, uncertainty, and a recommendation/);
		expect(merge).toContain("Review the entire combined change before finalization, including auto-merged paths");
		expect(merge).toContain("stale references, incompatible assumptions, broken cross-file contracts");
		expect(merge).toContain("Run the applicable non-mutating project-native checks");
		expect(merge.indexOf("Review the entire combined change")).toBeLessThan(
			merge.indexOf("Finalize the local merge only after"),
		);
	});

	it("reuses an owned finalized result and authorizes only the verified upstream publication", () => {
		const merge = section("## Step 4:", "## Step 5:");
		const publication = section("## Step 5:", "## Step 6:");
		expect(publication).toContain(
			"upstream repository and branch still equal the canonical repository and current branch",
		);
		expect(merge).toContain("create no empty or duplicate merge");
		expect(merge).toContain("Revalidate that the existing finalized result owns the intended integration");
		for (const phrase of [
			"exact upstream destination",
			"finalized local commit",
			"integrated incoming commit",
			"publication consequences",
			"consequences of retaining the result locally",
			"material uncertainty",
			"get_scramjet_user_input",
			"explicitly declined",
			"do not report terminal status in that turn",
			"verified configured upstream",
			"without asking again",
			"local `HEAD`, the configured upstream, the forge branch, and the matching PR head",
		]) {
			expect(publication).toContain(phrase);
		}
		expect(publication).not.toMatch(/`git push (?:--force|-f)\b/);
	});

	it("returns the complete delegated handoff while reserving lifecycle status for direct use", () => {
		const result = section("## Step 6:");
		for (const field of [
			"outcome: `integrated`, `already integrated`, `aborted`, `blocked`, or `indeterminate`",
			"repository, current feature branch, incoming branch, matching PR if any, upstream, merge base",
			"concise intent summaries for both merge-base deltas",
			"textual conflict resolutions and semantic follow-on corrections",
			"project-native checks and results",
			"merge commit, push destination, and local/upstream/forge/PR convergence evidence",
			"preserved unrelated state and unresolved follow-up",
		]) {
			expect(result).toContain(field);
		}
		expect(result).toMatch(/delegated invocation[\s\S]*do \*\*not\*\* call `report_scramjet_command_status`/);
		expect(result).toMatch(/direct invocation[\s\S]*After delivering your answer[\s\S]*omit `next_steps`/);
		expect(result).toContain('Report `status: "completed"` only');
		expect(result).toContain('Report `status: "blocked"`');
		expect(result).toContain('Report `status: "incomplete"`');
	});
});

describe("mach12 pre-merge version propagation contract", () => {
	const guidelines = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:find-contribution-guidelines.md`), "utf-8");
	const preMerge = readFileSync(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-pre-merge.md`), "utf-8");
	const version = preMerge.slice(preMerge.indexOf("### 7b."), preMerge.indexOf("### 7c."));
	const commit = preMerge.slice(preMerge.indexOf("## Step 8:"), preMerge.indexOf("## Step 9:"));

	it("consults all applicable contribution and release guidance before fallback investigation", () => {
		expect(guidelines).toMatch(/all applicable[^.]*contribution[^.]*release instructions/i);
		expect(guidelines).toMatch(/release[^.]*directly referenced/i);
		expect(guidelines).toMatch(/source paths/i);
		expect(guidelines).toMatch(/conflicts|missing details/i);
		expect(guidelines.indexOf("contribution and release guidance")).toBeLessThan(
			guidelines.indexOf("project scripts"),
		);
	});

	it("reads release authority after checkout and freshness handling", () => {
		const checkout = preMerge.indexOf("gh pr checkout <pr-number>");
		const freshness = preMerge.indexOf("## Step 4: Check branch freshness");
		const guidance = preMerge.indexOf("/mach12:find-contribution-guidelines");
		expect(checkout).toBeGreaterThan(-1);
		expect(freshness).toBeGreaterThan(checkout);
		expect(guidance).toBeGreaterThan(freshness);
	});

	it("runs applicable version generation or synchronization before commit", () => {
		expect(version).toMatch(/every applicable[^.]*generation or synchronization/i);
		expect(version).toMatch(/before (?:the )?commit/i);
		expect(preMerge.indexOf("generation or synchronization")).toBeLessThan(preMerge.indexOf("## Step 8:"));
	});

	it("verifies and commits the complete version change together", () => {
		expect(version).toMatch(/canonical version[^.]*required mirrors[^.]*tracked generated metadata/i);
		expect(version).toMatch(/repository-defined consistency checks/i);
		expect(commit).toMatch(/canonical version[^.]*required mirrors[^.]*tracked generated metadata/i);
		expect(commit).toMatch(/same (?:bounded )?(?:pre-merge )?commit|commit[^.]*together/i);
	});

	it("investigates incomplete authority and asks rather than guessing", () => {
		expect(version).toMatch(/guidance[^.]*absent|guidance[^.]*incomplete/i);
		expect(version).toMatch(/tracked files[^.]*project (?:scripts|commands)/i);
		expect(version).toMatch(/ask the user/i);
		expect(version).toMatch(/incomplete/i);
		expect(version).toMatch(/do not guess|rather than guessing|never infer/i);
	});

	it("preserves mandatory investigation and repository version authority", () => {
		expect(preMerge).toMatch(/after 7b's required authority and fallback investigation[^.]*optional/i);
		expect(version).toMatch(/follow the repository authority[^.]*version target or classification rule/i);
		expect(version).toMatch(/only when[^.]*semantic versioning[^.]*leaves the bump level unresolved/i);
		expect(version).toMatch(/ask the user rather than overriding repository authority/i);
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
		expect(readiness).toContain("A behind branch continues to Step 4");
		expect(readiness).toContain("pending or failing checks continue to Step 9");
		expect(readiness).toContain("`CONFLICTING` or `DIRTY`");
		expect(readiness).toContain("continue through checkout to Step 4");
		expect(readiness).toContain("does not authorize an automatic merge");
	});

	it("pre-merge authorizes integration before delegating and consumes only a verified success", () => {
		const freshness = preMerge.slice(
			preMerge.indexOf("## Step 4: Check branch freshness"),
			preMerge.indexOf("## Step 5: Read contribution guidelines"),
		);
		const mergeChoice = freshness.indexOf("**Merge**");
		const cancelChoice = freshness.indexOf("**Cancel**");
		const delegation = freshness.indexOf("/mach12:integrate-branch <default-branch> --pr <pr-number>");
		expect(mergeChoice).toBeGreaterThan(-1);
		expect(cancelChoice).toBeGreaterThan(mergeChoice);
		expect(delegation).toBeGreaterThan(cancelChoice);
		expect(freshness).toContain("get_scramjet_user_input");
		expect(freshness).toContain("prior informed **Merge** choice authorizes the delegated integration and push");
		expect(freshness).toContain("outcome is `integrated` or `already integrated`");
		expect(freshness).toContain("every required check succeeded");
		expect(freshness).toContain("local, upstream, forge, and matching-PR convergence is established");
		expect(freshness).toContain("For `aborted`, `blocked`, or `indeterminate` outcomes");
		expect(freshness).toContain("without attempting to integrate, finalize, or push it again");
		expect(freshness).toContain("delegated command exclusively owns integration");
		expect(freshness).not.toContain("git merge");
		expect(freshness).not.toContain("git checkout --theirs");
		expect(freshness).not.toContain("git commit --no-edit");
		expect(freshness).not.toMatch(/`git push(?:\s|`)/);

		const parsed = parseCommandFile(join(MACH12_COMMANDS_DIR, `${SET_NAME}:pr-pre-merge.md`), preMerge, SET_NAME);
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		expect(parsed.def.allowedTools).toContain("get_scramjet_user_input");
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
			finalSection.indexOf('- Report `status: "blocked"`'),
		);
		expect(finalSection).toContain("exactly three entries");
		expect(mergeEntry).toContain("`fresh_session`: `true`");
		expect(mergeEntry).toContain("non-empty reason explaining that the PR is merge-ready");
		expect(reviewEntry).toContain("`fresh_session`: `true`");
		expect(reviewEntry).toContain("non-empty reason explaining that additional static review is optional");
		expect(validationEntry).toContain("`fresh_session`: `true`");
		expect(validationEntry).toContain("non-empty reason explaining that executable validation is optional");
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
