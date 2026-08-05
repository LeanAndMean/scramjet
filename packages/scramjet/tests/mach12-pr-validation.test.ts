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
	{
		basename: "pr-validation-assessment",
		argumentHint: "<pr-number> --review-comment <id> --review-sha256 <digest> [context]",
	},
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

function routeMessages(content: string) {
	return [...content.matchAll(/`message`: `([^`]+)`/g)].map((match) => match[1]);
}

describe("mach12 executable PR validation command fixtures", () => {
	it.each(COMMANDS)("parses the $basename command fixture", ({ basename, argumentHint }) => {
		const { content, result } = readCommand(basename);

		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.argumentHint).toBe(argumentHint);
		expect(content.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(content).toContain("<user-context>\n$ARGUMENTS\n</user-context>");
		expect(content).toContain('agentScope: "user"');
	});
});

describe("mach12 pr-validation executable-proof workflow", () => {
	const command = readCommand("pr-validation").content;

	it("gates all mutation behind exact clean-head preflight and authoritative context", () => {
		const preflight = section(command, "## Step 2:", "## Step 3:");
		expect(preflight).toContain("The PR is open");
		expect(preflight).toContain("non-detached");
		expect(preflight).toContain("local feature branch");
		expect(preflight).toContain("headRefOid");
		expect(preflight).toContain("local `HEAD`");
		expect(preflight).toContain("empty index");
		expect(preflight).toContain("tracked and untracked");
		expect(preflight).toContain(
			"without stashing, resetting, cleaning, checking out, rebasing, pulling, or overwriting",
		);
		expect(preflight).toContain("PR head OID");
		expect(preflight).toContain("base OID");
		expect(preflight).toContain("actual merge-base OID");
		expect(preflight).toContain("initial clean status");

		const context = section(command, "## Step 3:", "## Step 4:");
		for (const source of [
			"title, body, base and head identities, files, commits, and all top-level PR conversation comments",
			"linked issue",
			"<!-- mach12-plan -->",
			"later amendments, decisions, and review-fix progress",
			"complete merge-base-to-head diff",
			"tests adjacent to every changed production boundary",
			"prior review, assessment, decision, and fix artifacts",
		]) {
			expect(context).toContain(source);
		}
		expect(context).toContain("untrusted evidence");
		expect(context).toContain("delegate for those comments");
		expect(context).toContain("/mach12:gh-pr-read <pr-number>");
		expect(context).toContain("For each issue, delegate");
		expect(context).toContain("/mach12:gh-issue-read <issue-number>");
		expectInOrder(command, "## Step 2:", "## Step 3:", "## Step 4:", "Implement the test");
	});

	it("partitions production risk before one bounded parallel designer fan-out", () => {
		const partition = section(command, "## Step 4:", "## Step 5:");
		expect(partition).toContain("test-only changes");
		expect(partition).toContain("coverage evidence");
		expect(partition).toContain("exactly one primary behavioral cluster");
		expect(partition).toContain("one explicit integration cluster");
		expect(partition).toContain("up to six");
		expect(partition).toContain("ceiling, not a quota");
		expect(partition).toContain("single parallel `subagent` call");
		expect(partition).toContain("one highest-value candidate");
		expect(partition).toContain('agentScope: "user"');
		expect(partition).toContain("unreviewed boundaries");
		for (const failure of [
			"(no output)",
			"malformed result",
			"duplicate cluster ID",
			"unexpected ID",
			"missing ID",
		]) {
			expect(partition).toContain(failure);
		}
		expect(partition).toContain("stop the validation workflow as incomplete");
		expectInOrder(partition, "Partition", "Dispatch", "Require exactly one", "De-duplicate");
	});

	it("owns candidates sequentially with a complete ledger and exact merge-base classification", () => {
		const candidates = section(command, "## Step 5:", "## Step 6:");
		for (const field of [
			"candidate and cluster IDs",
			"hypothesis and contract source",
			"fixture, assertion, and competing causes",
			"expected head and base behavior",
			"temporary and proposed permanent paths",
			"head and merge-base results",
			"assessor verdict",
			"narrowing allowance",
			"final disposition",
			"final path, node ID, finding ID, classification, and ownership group",
		]) {
			expect(candidates).toContain(field);
		}
		expect(candidates).toContain("one at a time");
		expect(candidates).toContain("assertion failure");
		expect(candidates).toContain("setup, runner, dependency, environment, or flaky failure");
		expect(candidates).toContain("one command-owned detached temporary worktree");
		expect(candidates).toContain("actual merge-base OID");
		expect(candidates).toContain("only the candidate's test delta");
		expect(candidates).toContain("immutable/frozen install mode");
		expect(candidates).toContain("tracked contents remain byte-for-byte unchanged");
		expect(candidates).toContain("bootstrap/environment failure");
		expect(candidates).toContain("Construct every executable invocation locally");
		expect(candidates).toMatch(
			/Resolve each test path, require its real path to remain inside the applicable worktree, and reject NULs, newlines, runner-option injection/,
		);
		expect(candidates).toMatch(
			/argv-capable fixed wrapper.+positional parameter.+quoted `"\$@"` argument—never concatenate values into shell source/,
		);
		expect(candidates).toContain("`--` before test paths wherever the runner supports an option boundary");
		expect(candidates).toContain("Never execute or interpolate a command string");
		expect(candidates).toMatch(/never switch or reset the primary worktree/i);
		expect(candidates).toContain("**head red / base green**: a PR regression candidate");
		expect(candidates).toMatch(
			/\*\*base inapplicable\*\*:.+approved plan, linked issue, or public contract; never call it a regression/,
		);
		expect(candidates).toContain(
			"**equivalent head / base red**: a pre-existing observation excluded from PR fix handoff",
		);
		expect(candidates).toMatch(/\*\*inconclusive\*\*:.+cannot support a valid comparison/);
	});

	it("requires independent admission, one narrowing round, and an exhaustive disposition", () => {
		const candidates = section(command, "## Step 5:", "## Step 6:");
		expect(candidates).toContain("mach12:independent-assessor");
		expectInOrder(
			candidates,
			"Keep the current candidate delta",
			"snapshot the exact primary and detached-worktree diffs",
			"rerun both locally constructed focused invocations sequentially without mutation",
			"compare both worktrees byte-for-byte",
			"reverse only the candidate's delta",
		);
		expect(candidates).toContain("content hashes for every untracked file");
		expect(candidates).toContain("A matching dirty-path set alone is insufficient");
		for (const check of [
			"fixture realism",
			"production reachability",
			"contract authority",
			"existing coverage",
			"merge-base evidence",
			"claimed-path sensitivity",
			"root-cause confidence",
			"approved-plan scope",
			"practical impact",
		]) {
			expect(candidates).toContain(check);
		}
		expect(candidates).toContain("exactly one focused narrowing round");
		expect(candidates).toContain("one final reassessment");
		for (const disposition of [
			"retained validated finding",
			"removed passing test",
			"removed invalid fixture or intended behavior",
			"removed duplicate or existing coverage",
			"removed pre-existing issue",
			"removed inconclusive or environmental observation",
			"rejected before implementation",
		]) {
			expect(candidates).toContain(disposition);
		}
		expect(candidates).toContain("Only independently validated PR-head-red findings");
		expectInOrder(command, "mach12:independent-assessor", "## Step 6:", "<!-- mach12-review -->");
	});

	it("normalizes only stable red proofs and dispatches architects after root causes settle", () => {
		const normalization = section(command, "## Step 6:", "## Step 7:");
		expect(normalization).toContain("established permanent behavioral suite");
		expect(normalization).toContain("stable subsystem-oriented name");
		for (const forbidden of [
			"PR numbers",
			"issue numbers",
			"finding IDs",
			"`probe`",
			"`review`",
			"`review_fix`",
			"`postfix`",
		]) {
			expect(normalization).toContain(forbidden);
		}
		expectInOrder(
			normalization,
			"When one or more findings are retained",
			"all retained nodes together sequentially",
			"Remove every temporary file and rejected candidate hunk",
			"mach12:code-architect",
			"Before delegating a retained run",
			"When zero findings are retained",
		);
		expect(normalization).toContain("exactly the expected failures");
		expect(normalization).toContain("targeted edits only");
		expect(normalization).toContain("dirty paths consist exactly of the declared normalized proof paths");
		expect(normalization).toContain("no production diff");
		expect(normalization).toContain("temporary investigation content");
		expect(normalization).toMatch(/retained node ID.+reproducibly red/);
		expect(normalization).toContain("minimal production fix proposals");
		expect(normalization).toContain("do not mutate");
		expect(normalization).toContain("require a clean primary worktree");
		expect(normalization).toContain("skip final-node and consolidated-red execution");
		expect(normalization).toContain("skip architect dispatch");
		expect(normalization).toContain("verified no-findings review artifact");
		expect(normalization).toContain("compare the complete diff and untracked contents byte-for-byte");
		expect(normalization).toContain("user-visible progress record");
		expect(normalization).toContain(
			"every controlled normal, failure, stop, assessor-error, and user-cancellation path that returns control",
		);
		expect(normalization).toMatch(
			/Abrupt cancellation or process termination cannot run later Markdown cleanup; the durable path record is the recovery contract/,
		);
		expect(normalization).toMatch(
			/resumed or next session must report the recorded paths, their state, and the exact manual recovery command/,
		);
		expect(normalization).toContain("exact manual recovery command");
		expect(normalization).toContain("a path-only check is insufficient");
	});

	it("persists one pushed proof commit for every retained run before publication", () => {
		const normalization = section(command, "## Step 6:", "## Step 7:");
		expect(normalization).toContain("exactly one proof commit for the complete retained validation run");
		expect(normalization).toContain("regardless of ownership-group count");
		expect(normalization).toContain("/mach12:push");
		expect(normalization).toContain("initial validation-proof payload");
		for (const field of [
			"repository and PR identity",
			"head branch",
			"frozen implementation parent",
			"exact proof paths",
			"path, node, finding, and ownership-group mapping",
			"intentional-red designation",
		]) {
			expect(normalization).toContain(field);
		}
		expect(normalization).toContain("must not require a review or assessment comment ID or digest");
		expectInOrder(
			normalization,
			"all retained nodes together sequentially",
			"initial validation-proof payload",
			"/mach12:push",
			"independently verify",
		);
	});

	it("creates no proof commit for zero findings and treats red proofs as non-merge-ready evidence", () => {
		const normalization = section(command, "## Step 6:", "## Step 7:");
		expect(normalization).toContain("create no commit and do not delegate to `/mach12:push`");
		expect(normalization).toContain("unchanged frozen implementation parent");
		expect(normalization).toContain("intentionally failing evidence");
		expect(normalization).toContain("does not claim successful CI or merge readiness");
		const publication = section(command, "## Step 7:");
		expect(publication).toContain("For a zero-finding run, record `proof commit: none`");
		expect(publication).toContain("require the marker, `proof commit: none`, and unchanged frozen implementation parent");
		expect(publication).toContain("proof commit or zero-finding unchanged parent");
	});

	it("verifies the pushed proof boundary before preparing or publishing the review", () => {
		expectInOrder(
			command,
			"/mach12:push",
			"local `HEAD`, its upstream branch, and a fresh GitHub `headRefOid` all equal the returned proof commit",
			"### Prepare the review artifact",
			"### Post the review artifact",
			"### Verify publication and report status",
		);
		const publication = section(command, "## Step 7:");
		for (const field of [
			"frozen implementation parent",
			"proof commit",
			"actual merge base",
			"path, node, finding, and ownership-group mapping",
			"expected red results",
			"publisher login",
		]) {
			expect(publication).toContain(field);
		}
		expect(publication).toContain("Git commit and declared proof mapping are the executable authority");
	});

	it("preserves a pushed proof commit across publication reconciliation", () => {
		const publication = section(command, "## Step 7:");
		expect(publication).toContain("If the proof push succeeded but publication failed or is ambiguous");
		expect(publication).toContain("preserve the exact pushed proof commit");
		expect(publication).toContain("search for the intended artifact");
		expect(publication).toContain("repository, PR, trusted publisher, frozen implementation parent, and proof commit identities");
		expect(publication).toContain("never recommit, repush, force-push, or blindly duplicate the comment");
	});

	it("publishes a verified fix-compatible review artifact before the fresh forced handoff", () => {
		const publication = section(command, "## Step 7:");
		expect(publication).toContain("`<!-- mach12-review -->` as the first line");
		expect(publication).toContain("same-repository issue or pull-request references use `#N`");
		expect(publication).toContain("cross-repository references use `owner/repo#N`");
		expect(publication).toContain("Artifact-local identifiers use stable labels or plain words");
		expect(publication).toContain("cluster IDs, node IDs");
		expect(publication).toContain("never bare `#N`");
		expect(publication).toContain("Preserve exact verified comment URLs and numeric provenance fields");
		expectInOrder(
			publication,
			"Before hashing or posting, format intentional GitHub relationships",
			"Prepare a fix-compatible review body",
			"Compute and record the SHA-256 digest",
			"/mach12:gh-comment pr <pr-number>",
		);
		for (const field of [
			"severity and production references",
			"final test path, node ID, and command",
			"expected and observed behavior",
			"head / merge-base classification",
			"root cause and confidence",
			"approved-plan scope defense",
			"practical trigger",
			"observer-visible consequence",
			"durable-state safety",
			"realistic frequency",
			"operational severity",
			"architect-informed fix direction",
		]) {
			expect(publication).toContain(field);
		}
		expect(publication).toContain("rejected-candidate disposition counts");
		expect(publication).toContain("consolidated red command and expected red results");
		expect(publication).toContain("frozen implementation parent");
		expect(publication).toContain("actual merge base");
		expect(publication).toContain("confirmation that the worktree is clean");
		expect(publication).toContain("/mach12:gh-comment pr <pr-number>");
		expect(publication).toContain("numeric comment ID");
		expect(publication).toContain("complete body exactly equals the prepared body");
		expect(publication).toContain(
			"search existing PR comments for one whose complete body exactly equals the prepared body",
		);
		expect(publication).toContain("marker and proof identity alone are insufficient");
		expect(publication).toMatch(/never blindly retry/i);
		expect(publication).toContain("`none — zero retained findings` result");
		expect(publication).toContain("include exactly one selector-visible context entry");
		expect(publication).toContain("SHA-256 digest of the exact complete prepared body");
		expect(publication).toContain("author equals the recorded authenticated publisher login");
		expect(publication).toContain(
			"`message`: `/mach12:pr-validation-assessment <pr-number> --review-comment <numeric-comment-id> --review-sha256 <body-sha256>`",
		);
		expect(publication).toContain("`fresh_session`: `true`");
		expect(publication).toContain(
			'`reason`: "Independently reassess the retained executable proofs in a fresh session."',
		);
		expect(publication).toContain("Set `recommended_next_step` to `0`");
		expectInOrder(publication, "Prepare", "Post", "Verify", "After delivering your answer");
	});
});

describe("mach12 pr-validation-assessment independent-proof workflow", () => {
	const command = readCommand("pr-validation-assessment").content;

	it("authenticates a clean immutable proof commit before assessment", () => {
		const handoff = section(command, "## Step 2:", "## Step 4:");
		for (const clause of [
			"frozen implementation parent `P`",
			"proof commit `V`",
			"Local `HEAD`, its upstream branch, and a fresh GitHub `headRefOid` all equal `V`",
			"`V` has exactly one parent equal to `P`",
			"`P..V` is tests-only",
			"clean index and tracked and untracked worktree",
		]) expect(handoff).toContain(clause);
		expect(handoff).not.toContain("expected dirty-path set");
		expect(handoff).not.toContain("complete proof-patch manifest");
	});

	it("executes committed nodes and handles base-inapplicable proofs without impossible replay", () => {
		const adjudication = section(command, "## Step 4:", "## Step 5:");
		expect(adjudication).toContain("Run every retained node directly at `V`");
		expect(adjudication).toContain("derive replay material from the immutable `P..V` proof diff");
		expect(adjudication).toContain("do not apply or execute the proof where the production surface is absent");
		expect(adjudication).toContain("surface is absent at the frozen base and introduced between the base and `P`");
		expect(adjudication).toContain("fails credibly at `V`");
		expect(adjudication).toContain('agentScope: "user"');
	});

	it("delegates one atomic rejected-group cleanup commit and verifies its durable boundary", () => {
		const cleanup = section(command, "## Step 5:", "## Step 6:");
		expect(command).toContain("one group-level keep or reject disposition for every ownership group");
		expect(command).toContain("a mixed surviving/rejected group is incomplete and stops before mutation");
		expect(cleanup).toContain("remove every member of each complete rejected ownership group");
		expect(cleanup).toContain("no member of a surviving group");
		expect(cleanup).toContain("surviving proof paths, nodes, assertions, and content remain unchanged");
		expect(cleanup).toContain("Delegate exactly one tests-only cleanup commit and push through `/mach12:push`");
		expect(cleanup).toContain("create no cleanup commit");
		expect(cleanup).toContain("local `HEAD`, its upstream branch, and a fresh GitHub `headRefOid`");
		expect(cleanup).toContain("never recommit, repush, force-push, or blindly duplicate a comment");
	});

	it("keeps zero findings commit-free and publishes authenticated current-head provenance", () => {
		const guards = section(command, "## Step 3:", "## Step 4:");
		expect(guards).toContain("`none — zero retained findings`");
		expect(guards).toContain("`proof commit: none`");
		expect(guards).toContain("Skip proof execution, detached resources, cleanup, and architect dispatch");
		const artifact = section(command, "## Step 6:", "## Step 7:");
		expect(artifact).toContain("original proof commit `V`");
		expect(artifact).toContain("current post-assessment head");
		expect(artifact).toContain("trusted repository, PR, publisher, `P`, original `V`, and current pushed-head identities");
		expect(artifact).toContain("provider normalization changed its body");
		expect(artifact).toContain("exact body equality remains required for completion");
		expect(artifact).toContain("preserve the exact pushed head");
		expect(artifact).toMatch(/never blindly retry/i);
	});

	it("emits complete fresh fix and pre-merge wires", () => {
		const routing = section(command, "## Step 7:");
		const messages = routeMessages(routing);
		expect(messages.some((message) => message.startsWith("/mach12:pr-review-fix"))).toBe(true);
		expect(messages.some((message) => message === "/mach12:pr-pre-merge <pr-number>")).toBe(true);
		for (const message of messages.filter((value) => value.startsWith("/mach12:pr-review-fix"))) {
			expect(message).toContain("--review-comment <review-id>");
			expect(message).toContain("--assessment-comment <assessment-id>");
			expect(message).toContain("--review-sha256 <review-digest>");
			expect(message).toContain("--assessment-sha256 <assessment-digest>");
		}
		expect(routing).toContain("`fresh_session`: `true`");
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
		]) {
			expect(command).toContain(clause);
		}
	});

	it.each(["gh-pr-read", "gh-issue-read"])(
		"%s exposes parent and comment timestamps with freshness guidance",
		(basename) => {
			const command = readFileSync(join(COMMANDS_DIR, `mach12:${basename}.md`), "utf-8");
			const request = section(command, "Request parent", "The query must declare");
			const result = section(command, "## Step 4: Return");

			expect(request).toContain("`title`, `body`, `createdAt`, `updatedAt`");
			expect(request).toContain("authorAssociation createdAt url");
			expect(result).toContain("`createdAt`, and `updatedAt`");
			expect(result).toContain("each comment's `createdAt`");
			expect(result).toContain("point-in-time evidence");
			expect(result).toContain("verify potentially stale material claims against current authoritative context");
			expect(result).toContain("never treat age alone as proof of invalidity");
		},
	);

	it("requires planning and review to reassess stale claims against task-specific authority", () => {
		const issuePlan = readFileSync(join(COMMANDS_DIR, "mach12:issue-plan.md"), "utf-8");
		const prReview = readFileSync(join(COMMANDS_DIR, "mach12:pr-review.md"), "utf-8");

		expect(issuePlan).toContain("parent `createdAt` and `updatedAt`");
		expect(issuePlan).toContain("Verify those claims against current repository authority");
		expect(prReview).toContain("--json title,body,createdAt,updatedAt,comments,files");
		expectInOrder(
			prReview,
			"Identify linked issues from explicit relationship forms",
			"contextually relevant bare `#<number>` references in the PR body",
			"references found only in the conversation as candidates",
			"Deduplicate issue numbers",
			"Before briefing reviewers",
			"/mach12:gh-issue-read <issue-number>",
		);
		expect(prReview).toContain("If any linked issue cannot be read completely");
		expect(prReview).toContain("stop before reviewer dispatch");
		expect(prReview).toContain("report the review blocked or incomplete");
		expect(prReview).toContain("linked issue identified under the relationship and contextual-relevance rules above");
		expect(prReview).toContain(
			"checked-out PR head, current diff, tests, linked-issue evidence, and repository guidance",
		);
		expect(prReview).toContain("Relevant artifact timestamps, identified freshness caveats");
	});

	it("reads timestamped plausible duplicate candidates before confident issue classification", () => {
		const issueCreate = readFileSync(join(COMMANDS_DIR, "mach12:issue-create.md"), "utf-8");
		const duplicateCheck = section(issueCreate, "## Step 10:", "## Step 11:");

		expect(duplicateCheck).toContain("--json number,title,state,url,createdAt,updatedAt");
		const plausibleMatches = section(duplicateCheck, "- **Plausible matches**", "After those checks:");
		expect(plausibleMatches).toContain(
			"Before confidently classifying any candidate as a duplicate or recommending linkage",
		);
		expect(plausibleMatches).toContain("/mach12:gh-issue-read <candidate-number>");
		expect(plausibleMatches).toContain("Track which candidates were read completely");
		expect(plausibleMatches).toContain("If a read fails");
		expect(plausibleMatches).toContain(
			"exclude that unread candidate from duplicate classification and every mention, comment, or linkage target",
		);
		expect(plausibleMatches).toContain("old age is insufficient proof that it is obsolete");

		const choices = section(duplicateCheck, "After those checks:");
		expect(choices).toContain("Only a successfully read candidate can be a clear duplicate");
		expect(choices).toContain(
			"If every candidate is unread, offer only retry, create without mentioning matches, or skip",
		);
		expect(choices).toContain("unread candidates must not be offered");
	});
});

describe("mach12 tool-scope authoring contract", () => {
	const guide = readFileSync(resolve(HERE, "..", "docs", "command-authoring.md"), "utf-8");

	it("documents delegated advisory scope and explicit subagent allowlists", () => {
		expect(guide).toContain("only while a delegated frame is active");
		expect(guide).toContain("top-level command calls before delegation are not warned");
		expect(guide).toContain("An absent or empty list leaves the child unrestricted");
		expect(guide).toContain("Read-only agents must declare an explicit non-empty allowlist");
		expect(guide).not.toContain("  - glob");
	});
});

describe("mach12 validation route contract", () => {
	it("validates closed-policy command names for complete cleanup wires", () => {
		const policy = {
			mode: "closed" as const,
			candidates: [{ name: "mach12:pr-review-fix" }, { name: "mach12:pr-pre-merge" }],
		};
		const commandCheck = (name: string) =>
			["mach12:pr-review-fix", "mach12:pr-pre-merge"].includes(name) ? null : `${name} is unknown`;
		const cleanup = {
			message:
				"/mach12:pr-review-fix 428 --review-comment 1 --assessment-comment 2 --review-sha256 aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --assessment-sha256 bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb --cleanup-finding S1",
			fresh_session: true,
			reason: "Remove the declined red proof before pre-merge.",
		};
		const result = validateNextSteps(
			[cleanup, { ...cleanup, message: "not a command" }, { ...cleanup, message: "/unknown:command" }],
			policy,
			0,
			commandCheck,
		);

		expect(result.valid.map((step) => step.message)).toEqual([cleanup.message]);
		expect(result.skipped.map((step) => step.reason)).toEqual([
			"non-command messages are valid only for open policies",
			"unknown:command is not in closed candidates [mach12:pr-review-fix, mach12:pr-pre-merge]",
		]);
		expect(result.recommended?.message).toBe(cleanup.message);
	});
});

describe("mach12 executable validation integration", () => {
	const prCreate = readFileSync(join(COMMANDS_DIR, "mach12:pr-create.md"), "utf-8");
	const prReviewFix = readFileSync(join(COMMANDS_DIR, "mach12:pr-review-fix.md"), "utf-8");
	const push = readFileSync(join(COMMANDS_DIR, "mach12:push.md"), "utf-8");
	const testDesigner = readFileSync(resolve(HERE, "..", "mach12", "agents", "mach12:test-designer.md"), "utf-8");

	it("makes the PR-validation designer result exclusive of the general output format", () => {
		const specialized = section(testDesigner, "For a PR-validation brief", "## Core Responsibilities");
		expect(specialized).toContain("return exactly one candidate");
		expect(specialized).toContain("and no other output, then stop");

		const general = section(testDesigner, "## Output Format", "## Quality Principles");
		expect(general).toContain("only for briefs other than PR validation");
	});

	it("offers ordinary review first and recommends it over opt-in executable validation after PR creation", () => {
		const reporting = section(prCreate, "## Step 5:");
		expectInOrder(reporting, "/mach12:pr-review <pr-number>", "/mach12:pr-validation <pr-number>");
		expect(reporting).toContain("slower, opt-in executable-behavior path");
		expect(reporting).toContain("Set `recommended_next_step` to `0`, ordinary PR review");
	});

	it("keeps review-cycle history evidence-conservative and current artifacts authoritative", () => {
		const context = section(prReviewFix, "## Step 2:", "## Step 3:");
		expect(context).toContain("complete verified chronological top-level PR comment stream");
		expect(context).toContain("match exactly one recognized review");
		expect(context).toContain("before selecting findings or publishing the ID as provenance");
		expect(context).toContain("literal `<!-- mach12-review -->` marker");
		expect(context).toContain(
			"structured review format with Critical/Important/Suggestions sections and model attribution",
		);
		expect(context).toContain("literal `<!-- mach12-assessment -->` marker");
		expect(context).toContain("literal `<!-- mach12-progress -->` marker");
		expect(context).toContain("Recognition determines retrospective inventory only");
		expect(context).toContain("does not authenticate an artifact or associate it with a cycle");
		expect(context).toContain("review comment's numeric ID");
		expect(context).toContain("explicitly references that review comment ID or URL");
		expect(context).toContain("existing validation provenance");
		expect(context).toMatch(/chronology, authorship, matching prose, or reused F\/S identifiers/i);
		expect(context).toContain("leave it unassociated");
		expect(context).toContain("F/S identifiers are scoped to their originating review comment");
		expect(context).toContain("exact invocation-selected review and optional assessment");
		expect(context).toContain("must not replace or reinterpret them");
	});

	it("preserves the originating review ID across ordinary fix progress publication", () => {
		const handoff = section(prReviewFix, "## Step 5:");
		expect(handoff).toContain("ordinary static-review repair");
		expect(handoff).toContain("exact resolved numeric review comment ID");
		expect(handoff).toContain("originating review ID");

		const comment = section(push, "### Comment content", "## Step 5:");
		expect(comment).toContain("ordinary static-review repair");
		expect(comment).toContain("exact numeric review comment ID supplied by the caller");
		expect(comment).toContain("originating review ID");
		expect(comment).toContain("does not constitute validation provenance");

		expect(comment).toContain(
			"For an ordinary repair, preserve the exact originating review ID supplied by the caller",
		);
		expect(comment).toContain(
			"For a validation-origin repair, preserve the already-validated structured provenance payload verbatim",
		);
	});

	it("requires an evidence-grounded review retrospective without weakening current-session reporting", () => {
		const summary = section(prReviewFix, "7. **Summary**", "Treat the selected findings list");
		expectInOrder(
			summary,
			"Lead verdict",
			"Review-cycle progression",
			"This fix session",
			"Overall trajectory",
			"Current blockers and residual scope",
			"Recommendation",
		);
		expect(summary).toContain("refresh the PR's head OID, commit history, and checks");
		expect(summary).toContain("refreshed `headRefOid` to equal the verified pushed `HEAD`");
		expect(summary).toMatch(
			/head mismatch or unavailable, pending, cancelled, or failed checks as unresolved evidence/i,
		);
		expect(summary).toContain("cannot support a readiness claim");
		expect(summary).toMatch(/converging, stalled, regressing, or blocked/);
		expect(summary).toContain("one chronological entry per recognizable review cycle");
		expect(summary).toContain("complete the full progression before the next section");
		expect(summary).toContain("actual concerns or theme");
		expect(summary).toContain("exact invocation-selected cycle");
		expect(summary).toContain("after the complete review-cycle progression");
		expect(summary).toContain("explicit temporal boundary");
		expect(summary).toContain("cycles after the invoked review as subsequent");
		expect(summary).toContain("not used as authority for this fix");
		expect(summary).toContain("no other review cycle was recognized");
		expect(summary).toContain("substantively analyze the invoked cycle");
		for (const detail of [
			"selected findings",
			"completed changes",
			"files modified",
			"key decisions",
			"tests and results",
			"commit/push outcome",
			"progress-comment outcome",
			"remaining staged work",
		]) {
			expect(summary).toContain(detail);
		}
		expect(summary).toContain("cross-cycle synthesis, not a list of completed actions");
		expect(summary).toContain("findings are becoming narrower or deeper");
		expect(summary).toContain("behavioral defect from a mechanical gate");
		expect(summary).toContain("Never report a bare F/S identifier or classification");
		expect(summary).toContain("Immediately restate the finding's one-line description");
		expect(summary).toContain("state when the verified record cannot support a conclusion");
		expect(summary).toContain("cannot expand the bounded finding scope");
		expect(summary).toContain("weaken validation-origin authentication");
	});

	it("authenticates staged validation repairs through an exact predecessor chain", () => {
		const authentication = section(
			prReviewFix,
			"### Validation-origin artifact authentication",
			"### Validation-origin proof contract",
		);
		expect(authentication).toContain("For the first repair session");
		expect(authentication).toContain("Do not accept `--predecessor-head`");
		expect(authentication).toContain("For a staged continuation, require `--predecessor-head`");
		expect(authentication).toContain("local `HEAD` and GitHub `headRefOid` must both equal the supplied predecessor");
		expect(authentication).toContain("trusted `<!-- mach12-progress -->` comment");
		expect(authentication).toContain("Walk backward through those trusted progress records");
		expect(authentication).toContain("rejecting gaps, forks, duplicate successors");

		const reporting = section(prReviewFix, "## Step 5:");
		expect(reporting).toContain("`--predecessor-head <pushed-head>`");
		expect(reporting).toContain("exact verified pushed head as `--predecessor-head`");
		expect(reporting).toContain("next stage's selected canonical IDs");
		expect(reporting).toContain(
			"explicit exhaustive ownership-group-safe partition of the predecessor's remaining staged IDs",
		);
		expect(reporting).toContain("Previously selected IDs are authenticated by the predecessor chain");

		const example = section(reporting, "   - Example after Stage 1", "   - Validation-origin continuation wires");
		expect(example).toContain("--review-comment 1234567890");
		expect(example).toContain("--assessment-comment 1234567891");
		expect(example).toContain(`--review-sha256 ${"a".repeat(64)}`);
		expect(example).toContain(`--assessment-sha256 ${"b".repeat(64)}`);
		expect(example).toContain(`--predecessor-head ${"c".repeat(40)}`);
		expect(example).toContain("--staged-later S4 --staged-later S5 F2 F3 Stage 2");
		expect(example).not.toMatch(/(?:^|\s)F1(?:\s|$)/);

		expect(reporting).toContain("structured provenance payload");
		expect(reporting).toContain("preserve every supplied provenance field verbatim");
	});

	it("preserves validation-origin proofs through an ordered red-to-green production repair", () => {
		const parse = section(prReviewFix, "## Step 1:", "## Step 2:");
		expect(parse).toContain("Repeatable **`--cleanup-finding <id>`** flags");
		expect(parse).toContain("Repeatable **`--staged-later <id>`** flags");
		expect(parse).toMatch(/Each repeatable cleanup or staged-later flag consumes exactly one following identifier/);
		expect(parse).toMatch(/Require trailing context to name the later repair stage when `--staged-later` is present/);
		expect(parse).toContain("`^(F|S)[1-9][0-9]*$`");
		expect(parse).toContain("prohibit bare numbers");
		expect(parse).toContain("pairwise disjoint");
		expect(parse).toContain(
			"reject combining any `--cleanup-finding` with production repair IDs or with `--staged-later`",
		);
		expect(parse).toContain(
			"For a first validation-origin repair, require their union to exhaust every surviving proof ID",
		);
		expect(parse).toContain(
			"For a staged continuation, require selected and staged-later IDs to exhaust exactly the predecessor chain's remaining staged IDs",
		);

		const implementation = section(prReviewFix, "## Step 4:", "## Step 5:");
		for (const clause of [
			"exact retained node IDs, proof constraints, authenticated proof-patch bodies and digests, and ownership groups for every surviving finding from both comments",
			"behavioral contracts, assertions, paths, and node IDs",
			"weakening assertions",
			"skipping or converting tests to expected failures",
			"accepting snapshots",
			"renaming or relocating paths or node IDs",
			"duplicating proof tests",
			"Partition the applicable disposition domain—every surviving proof for a first repair, or only the predecessor's remaining staged proofs for a continuation",
			"Preserve selected and staged-later proof patches unchanged",
			"Change production code, not retained proof tests",
			"Do not require proofs for unselected findings to become green in this session",
		]) {
			expect(implementation).toContain(clause);
		}
		expectInOrder(
			implementation,
			"Run every retained node associated with the selected findings before editing production code",
			"confirm its recorded red state",
			"Change production code, not retained proof tests",
			"Rerun the same selected retained nodes after the production edits",
			"confirm they are green",
			"broader focused suites",
		);
		expect(implementation).toContain("Ordinary static-review fixes retain their existing behavior");
		expect(implementation).toContain("require both exact comment IDs and both SHA-256 bindings");
		expect(implementation).toContain("--cleanup-finding");
		expect(implementation).toContain("For a first repair, partition every surviving proof");
		expect(implementation).toContain(
			"partition only the remaining staged proofs recorded at the supplied predecessor head",
		);
		expect(implementation).toContain("previously selected proofs remain authenticated chain history");
		expect(implementation).toContain("require cleanup IDs to exhaust every surviving proof");
		expect(implementation).toContain("no surviving red proof remains");
		expect(implementation).toContain("both artifact authors exactly equal the authenticated login");
	});

	it("gives authenticated cleanup a pre-merge-only completion route", () => {
		const reporting = section(prReviewFix, "## Step 5:");
		const cleanup = section(
			reporting,
			"- **Successful terminal `--cleanup-finding` run:**",
			"- **Production-repair run:**",
		);
		expect(cleanup.match(/`message`:/g)).toHaveLength(1);
		expect(cleanup).toContain("`message`: `/mach12:pr-pre-merge <pr-number>`");
		expect(cleanup).toContain("Set `recommended_next_step` to `0`");
		expect(cleanup).toContain("Do not offer review or validation after cleanup");
		expect(cleanup).not.toContain("/mach12:pr-review <pr-number>");
		expect(cleanup).not.toContain("/mach12:pr-validation <pr-number>");
	});

	it("keeps staged fixes on the fix command and exposes all final verification routes", () => {
		const reporting = section(prReviewFix, "## Step 5:");
		const intermediate = section(
			reporting,
			"1. **Continue staged fixing first.**",
			"2. **After the final planned fix stage",
		);
		expect(intermediate).toContain("`message`: `/mach12:pr-review-fix`");
		expect(intermediate).not.toContain("`message`: `/mach12:pr-validation");
		expect(intermediate).not.toContain("`message`: `/mach12:pr-pre-merge");

		const finalRoutes = section(reporting, "2. **After the final planned fix stage");
		expectInOrder(
			finalRoutes,
			"`message`: `/mach12:pr-review <pr-number>`",
			"`message`: `/mach12:pr-validation <pr-number>`",
			"`message`: `/mach12:pr-pre-merge <pr-number>`",
		);
		expect(finalRoutes).toContain("recommend `mach12:pr-validation` (index 1) for validation-origin repairs");
		expect(finalRoutes).toContain("recommend `mach12:pr-review` (index 0)");
		expect(finalRoutes).toContain("recommend `mach12:pr-pre-merge` (index 2)");
	});

	it("owns the initial validation-proof commit as an exact tests-only push mode", () => {
		const determine = section(push, "## Step 1:", "## Step 2:");
		expect(determine).toContain("distinct initial validation-proof payload");
		expect(determine).toContain("does not require review or assessment IDs or digests");
		for (const guard of [
			"`HEAD` equals the frozen implementation parent",
			"index is empty",
			"dirty path set exactly equals the declared proof paths",
			"tests-only",
			"temporary",
			"unrelated",
			"secret-bearing",
		]) {
			expect(determine).toContain(guard);
		}
		expect(determine).toContain("stage only the exact supplied proof paths");
		expect(determine).toContain("complete declared tests-only worktree diff");
		expect(determine).toContain("no unstaged residual content");

		const commit = section(push, "## Step 2:", "## Step 4:");
		expect(commit).toContain("exactly one commit");
		expect(commit).toContain("push exactly once");
		expect(commit).toContain("exactly one parent equal to the frozen implementation parent");
		expect(commit).toContain("local `HEAD`, the upstream branch, and a fresh GitHub `headRefOid`");
		expect(commit).toContain("clean index and tracked/untracked worktree");
		expect(commit).toContain("return the frozen implementation parent, proof commit, committed paths, and remote verification");
	});

	it("preserves staged repair provenance verbatim through the push subroutine", () => {
		expect(push).toContain("structured validation-origin provenance payload");
		expectInOrder(
			push,
			"validate it before staging, committing, or pushing",
			"Run `git status`",
			"## Step 2: Commit",
			"## Step 3: Push",
		);
		expect(push).toContain("stop before any repository or remote mutation");
		expect(push).toContain("preserve every field and value verbatim");
		expect(push).toContain("append the exact pushed `HEAD` as the predecessor head");
		expect(push).toContain("Do not summarize, reorder, omit, or rewrite these fields");
		expect(push).toContain("stop before posting the progress comment");
		expect(push).toContain("return an incomplete result to the caller with the exact pushed `HEAD`");
		expect(push).toContain("The top-level caller reports the workflow status");
		expect(push).toContain("without recommitting or repushing");
		expect(push).toContain("Never retry a commit or push as publication recovery");
	});

	it("requires one fixed-shape test-designer result for the assigned cluster", () => {
		const designer = readFileSync(resolve(HERE, "..", "mach12", "agents", "mach12:test-designer.md"), "utf-8");
		expect(designer).toContain("return exactly one candidate for the assigned cluster ID in this fixed shape");
		for (const field of [
			"**Cluster ID:**",
			"**Challenged behavior:**",
			"**Authority:**",
			"**Coverage gap:**",
			"**Fixture and assertion:**",
			"**Expected behavior:**",
			"**Production path:**",
			"**Permanent suite:**",
			"**Assessment:**",
		]) {
			expect(designer).toContain(field);
		}
		expect(designer).toMatch(/Do not return multiple candidates, omit the cluster ID, or substitute another ID/);
	});
});
