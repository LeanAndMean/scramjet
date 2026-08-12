import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";
import { validateNextSteps } from "../src/commands/validator.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const COMMANDS_DIR = resolve(HERE, "..", "mach12", "commands");
const COMMANDS = [
	{ basename: "pr-validation", argumentHint: "<pr-number> [context]" },
	{ basename: "pr-validation-assessment", argumentHint: "<pr-number> --review-comment <id> [context]" },
] as const;

function readCommand(basename: (typeof COMMANDS)[number]["basename"]) {
	const filePath = join(COMMANDS_DIR, `mach12:${basename}.md`);
	const content = readFileSync(filePath, "utf-8");
	return { content, result: parseCommandFile(filePath, content, "mach12") };
}

function section(content: string, start: string, end?: string) {
	const startIndex = content.indexOf(start);
	expect(startIndex, `missing section start: ${start}`).toBeGreaterThanOrEqual(0);
	const endIndex = end === undefined ? content.length : content.indexOf(end, startIndex + start.length);
	expect(endIndex, `missing section end: ${end}`).toBeGreaterThan(startIndex);
	return content.slice(startIndex, endIndex);
}

function expectInOrder(content: string, ...needles: string[]) {
	let previous = -1;
	for (const needle of needles) {
		const current = content.indexOf(needle);
		expect(current, `missing ordered clause: ${needle}`).toBeGreaterThan(previous);
		previous = current;
	}
}

describe("mach12 executable PR validation command fixtures", () => {
	it.each(COMMANDS)("parses the $basename command fixture", ({ basename, argumentHint }) => {
		const { content, result } = readCommand(basename);

		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.argumentHint).toBe(argumentHint);
		expect(content.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(content).toContain("<user-context>\n$ARGUMENTS\n</user-context>");
	});
});

describe("mach12 pr-validation candidate workflow", () => {
	const command = readCommand("pr-validation").content;

	it("starts from a clean current PR head and gathers authority before design", () => {
		const boundary = section(command, "## Step 1:", "## Step 3:");
		for (const clause of [
			"open PR",
			"matching non-detached local head branch",
			"local `HEAD` equal to the current GitHub PR head",
			"empty index",
			"no tracked or untracked changes",
			"implementation parent `P`",
			"actual merge base",
		])
			expect(boundary).toContain(clause);
		expectInOrder(command, "## Step 1:", "## Step 2:", "## Step 3:", "Dispatch focused `mach12:test-designer`");
	});

	it("uses read-only designers while the main agent owns sequential candidate mutation", () => {
		const candidates = section(command, "## Step 3:", "## Step 4:");
		expect(candidates).toContain('agentScope: "user"');
		expect(candidates).toContain("Designers are read-only");
		expect(candidates).toContain("The main agent owns all repository mutation and execution");
		expect(candidates).toContain("sequentially in the primary repository");
		expect(candidates).toContain("ordinary uncommitted test changes");
		expect(candidates).toContain("Do not modify production code");
		expect(candidates).toContain("Remove passing, invalid, duplicate, pre-existing, environmental");
	});

	it("allows useful baseline comparison without linked worktrees or primary-head changes", () => {
		const candidates = section(command, "## Step 3:", "## Step 4:");
		expect(candidates).toContain(
			"Compare with the actual merge base when that comparison would materially establish",
		);
		expect(candidates).toContain("Choose a safe isolated method");
		expect(candidates).toContain("do not use linked Git worktrees");
		expect(candidates).toContain("do not change the primary repository's branch or `HEAD`");
	});

	it("publishes preliminary claims before a same-session assessment without committing", () => {
		const publication = section(command, "## Step 4:");
		expectInOrder(
			publication,
			"Prepare a review body",
			"call `add_pr_comment`",
			"Do not normalize tests into final suites",
			"`message`: `/mach12:pr-validation-assessment <pr-number> --review-comment <numeric-comment-id>`",
		);
		expect(publication).toContain("preliminary claims awaiting independent assessment");
		expect(publication).not.toContain("Present the candidate results");
		expect(publication).toContain("Retain the verified review URL and numeric ID");
		expect(publication).toContain("`fresh_session`: `false`");
		expect(publication).not.toContain("/mach12:push");
	});
});

describe("mach12 pr-validation-assessment accepted-proof workflow", () => {
	const command = readCommand("pr-validation-assessment").content;

	it("authenticates the preliminary review and exact uncommitted candidate boundary", () => {
		const state = section(command, "## Step 1:", "## Step 2:");
		for (const clause of [
			"exact numeric `--review-comment` ID",
			"trusted author",
			"`<!-- mach12-review -->` marker",
			"local and GitHub heads to remain at implementation parent `P`",
			"index to remain empty",
			"all worktree changes to be the candidate test changes",
			"no production or unrelated changes",
		])
			expect(state).toContain(clause);
	});

	it("independently adjudicates before removing rejected tests and committing accepted proofs", () => {
		expectInOrder(
			command,
			"## Step 2: Independently assess every candidate",
			"Dispatch `mach12:independent-assessor`",
			"Classify each candidate as an accepted defect",
			"## Step 3: Finalize accepted proof tests",
			"Remove rejected candidate changes through targeted edits",
			"Normalize accepted tests",
			"Rerun every final accepted node",
			"delegate exactly one accepted-proof commit and push through `/mach12:push`",
		);
		expect(command).toContain("do not retain optional red proofs");
		expect(command).toContain("Do not repair production code");
	});

	it("does not repeat final candidate dispositions after assessment publication", () => {
		const publication = section(command, "## Step 4:", "## Step 5:");
		expect(publication).not.toContain("Present the final dispositions");
		expect(publication).toContain("Retain the verified assessment ID");
		expect(publication).toContain("accepted IDs for routing");
	});

	it("creates one accepted-only proof commit or no commit when nothing survives", () => {
		const finalization = section(command, "## Step 3:", "## Step 4:");
		expect(finalization).toContain("`P..V` contains exactly the accepted tests");
		expect(finalization).toContain("local `HEAD`, upstream, and fresh GitHub `headRefOid` all equal `V`");
		expect(finalization).toContain("When no candidate survives");
		expect(finalization).toContain("create no commit or push");
	});

	it("publishes one final artifact and routes all accepted defects together", () => {
		const publication = section(command, "## Step 4:", "## Step 5:");
		expect(publication).toContain("`<!-- mach12-assessment -->`");
		expect(publication).toContain("proof commit `V`, or `proof commit: none`");
		expect(publication).toContain("final accepted test paths, node IDs, assertions, expected failures");

		const routing = section(command, "## Step 5:");
		expect(routing).toContain("emit exactly one entry containing every accepted ID");
		expect(routing).toContain(
			"/mach12:pr-review-fix <pr-number> --review-comment <review-id> --assessment-comment <assessment-id> <all-accepted-ids>",
		);
		expect(routing).toContain("/mach12:pr-pre-merge <pr-number>");
	});
});

describe("mach12 validation simplification boundaries", () => {
	const validation = readCommand("pr-validation").content;
	const assessment = readCommand("pr-validation-assessment").content;
	const fix = readFileSync(join(COMMANDS_DIR, "mach12:pr-review-fix.md"), "utf-8");
	const push = readFileSync(join(COMMANDS_DIR, "mach12:push.md"), "utf-8");
	const combined = `${validation}\n${assessment}\n${fix}\n${push}`;

	it("removes the superseded proof transaction protocol", () => {
		for (const obsolete of [
			"mach12-assessment-checkpoint",
			"assessment-cleanup",
			"ownership-group",
			"--staged-later",
			"--predecessor-head",
			"--cleanup-finding",
			"per-path proof content SHA-256",
			"Walk backward through those trusted progress records",
		])
			expect(combined).not.toContain(obsolete);
	});

	it("keeps accepted proof publication atomic at the actual mutation boundary", () => {
		const accepted = section(
			push,
			"When the caller supplies an **accepted validation-proof** payload",
			"For ordinary work",
		);
		expectInOrder(
			accepted,
			"authenticated open PR",
			"empty index",
			"worktree changes consisting exactly",
			"Stage only the accepted test paths",
		);
		const commitAndPush = section(push, "## Step 2:", "## Step 4:");
		expectInOrder(
			commitAndPush,
			"create exactly one direct successor of `P`",
			"verify that it has sole parent `P`",
			"Push once",
			"verify local `HEAD`, upstream, and a fresh GitHub PR head agree",
		);
	});

	it("repairs every accepted proof through production changes", () => {
		const identification = section(fix, "## Step 3:", "## Step 4:");
		expect(identification).toContain("resolve accepted F/S identifiers and descriptions from the final assessment");

		const proof = section(fix, "### Validation-origin proof contract", "Ordinary static-review fixes retain");
		expectInOrder(
			proof,
			"Require every accepted defect",
			"Rerun every accepted proof node before editing",
			"Modify production code",
			"rerun every accepted proof green",
		);
		expect(proof).toContain("Do not weaken, skip, xfail, relocate, rename, duplicate, or edit proof tests");
	});
});

describe("mach12 authoritative GitHub history helpers", () => {
	it.each(["gh-pr-read", "gh-issue-read"])("%s paginates and verifies its comment stream", (basename) => {
		const command = readFileSync(join(COMMANDS_DIR, `mach12:${basename}.md`), "utf-8");
		for (const clause of [
			"gh api graphql --paginate",
			"comments(first: 100, after: $endCursor)",
			"totalCount",
			"pageInfo { hasNextPage endCursor }",
			"accumulated node count exactly equals `totalCount`",
			"reject duplicate database IDs",
			"do not return a partial array as complete",
		])
			expect(command).toContain(clause);
	});

	it.each(["gh-pr-read", "gh-issue-read"])("%s exposes timestamps with freshness guidance", (basename) => {
		const command = readFileSync(join(COMMANDS_DIR, `mach12:${basename}.md`), "utf-8");
		const request = section(command, "Request parent", "The query must declare");
		const result = section(command, "## Step 4: Return");
		expect(request).toContain("`title`, `body`, `createdAt`, `updatedAt`");
		expect(request).toContain("authorAssociation createdAt url");
		expect(result).toContain("point-in-time evidence");
		expect(result).toContain("verify potentially stale material claims against current authoritative context");
	});

	it("requires planning and review to reassess stale claims against current authority", () => {
		const issuePlan = readFileSync(join(COMMANDS_DIR, "mach12:issue-plan.md"), "utf-8");
		const prReview = readFileSync(join(COMMANDS_DIR, "mach12:pr-review.md"), "utf-8");
		expect(issuePlan).toContain("Verify those claims against current repository authority");
		expect(prReview).toContain(
			"checked-out PR head, current diff, tests, linked-issue evidence, and repository guidance",
		);
	});

	it("reads plausible duplicate candidates before issue classification", () => {
		const issueCreate = readFileSync(join(COMMANDS_DIR, "mach12:issue-create.md"), "utf-8");
		const duplicateCheck = section(issueCreate, "## Step 9:", "## Step 10:");
		expect(duplicateCheck).toContain("/mach12:gh-issue-read <candidate-number>");
		expect(duplicateCheck).toContain("Only a successfully read candidate can be a clear duplicate");
		expect(duplicateCheck).toContain("old age is insufficient proof that it is obsolete");
	});
});

describe("mach12 tool-scope authoring contract", () => {
	const guide = readFileSync(resolve(HERE, "..", "docs", "command-authoring.md"), "utf-8");

	it("documents delegated advisory scope and explicit subagent allowlists", () => {
		expect(guide).toContain("only while a delegated frame is active");
		expect(guide).toContain("An absent or empty list leaves the child unrestricted");
		expect(guide).toContain("Read-only agents must declare an explicit non-empty allowlist");
		expect(guide).not.toContain("  - glob");
	});
});

describe("mach12 validation route contract", () => {
	it("accepts the complete accepted-finding fix route", () => {
		const policy = {
			mode: "closed" as const,
			candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
		};
		const commandCheck = (name: string) =>
			["mach12:pr-review-fix", "mach12:pr-pre-merge"].includes(name) ? null : `${name} is unknown`;
		const fix = {
			message: "/mach12:pr-review-fix 428 --review-comment 1 --assessment-comment 2 F1 F2",
			fresh_session: true,
			reason: "Fix every accepted executable defect.",
		};
		const result = validateNextSteps(
			[fix, { ...fix, message: "not a command" }, { ...fix, message: "/unknown:command" }],
			policy,
			0,
			commandCheck,
		);
		expect(result.valid.map((step) => step.message)).toEqual([fix.message]);
		expect(result.recommended?.message).toBe(fix.message);
	});
});

describe("mach12 executable validation integration", () => {
	const prCreate = readFileSync(join(COMMANDS_DIR, "mach12:pr-create.md"), "utf-8");
	const prReviewFix = readFileSync(join(COMMANDS_DIR, "mach12:pr-review-fix.md"), "utf-8");
	const push = readFileSync(join(COMMANDS_DIR, "mach12:push.md"), "utf-8");
	const testDesigner = readFileSync(resolve(HERE, "..", "mach12", "agents", "mach12:test-designer.md"), "utf-8");

	it("keeps the PR-validation designer output exclusive and read-only", () => {
		const specialized = section(testDesigner, "For a PR-validation brief", "## Core Responsibilities");
		expect(specialized).toContain("return exactly one candidate");
		expect(specialized).toContain("and no other output, then stop");
		expect(testDesigner).toContain("structurally read-only");
	});

	it("offers ordinary review before opt-in executable validation after PR creation", () => {
		const reporting = section(prCreate, "## Step 5:");
		expectInOrder(reporting, "/mach12:pr-review <pr-number>", "/mach12:pr-validation <pr-number>");
		expect(reporting).toContain("slower, opt-in executable-behavior path");
	});

	it("preserves ordinary review-cycle evidence and originating review publication", () => {
		const context = section(prReviewFix, "## Step 2:", "## Step 3:");
		expect(context).toContain("complete verified chronological top-level PR comment stream");
		expect(context).toContain("Recognition determines retrospective inventory only");
		expect(context).toContain("exact invocation-selected review and optional assessment");
		const handoff = section(prReviewFix, "## Step 5:");
		expect(handoff).toContain("exact originating review ID");
		expect(push).toContain("Preserve an exact originating review ID");
	});

	it("captures verified comment identity before routing", () => {
		const validation = readFileSync(join(COMMANDS_DIR, "mach12:pr-validation.md"), "utf-8");
		expect(validation).toContain("call `add_pr_comment`");
		expect(validation).toContain("extract the numeric GitHub comment ID");
		expect(validation).toContain("re-fetch that exact comment");
		expect(validation).toContain("ambiguity prohibits automatic retry");
	});

	it("keeps final review, validation, and pre-merge choices after fixes", () => {
		const reporting = section(prReviewFix, "After delivering your answer");
		expectInOrder(
			reporting,
			"/mach12:pr-review <pr-number>",
			"/mach12:pr-validation <pr-number>",
			"/mach12:pr-pre-merge <pr-number>",
		);
	});
});
