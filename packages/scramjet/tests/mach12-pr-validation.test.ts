import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const COMMANDS_DIR = resolve(HERE, "..", "mach12", "commands");
const COMMANDS = [
	{ basename: "pr-validation", argumentHint: "<pr-number> [context]" },
	{
		basename: "pr-validation-assessment",
		argumentHint: "<pr-number> --review-comment <id> [context]",
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
		expect(preflight).toContain("open");
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
			"title, body, base and head identities, files, commits, and complete comments",
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
		expect(context).toContain("delegate for the full comment stream");
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
		expectInOrder(partition, "Partition", "Dispatch", "De-duplicate");
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
			"final path, node ID, and finding ID",
		]) {
			expect(candidates).toContain(field);
		}
		expect(candidates).toContain("one at a time");
		expect(candidates).toContain("assertion failure");
		expect(candidates).toContain("setup, runner, dependency, environment, or flaky failure");
		expect(candidates).toContain("one command-owned detached temporary worktree");
		expect(candidates).toContain("actual merge-base OID");
		expect(candidates).toContain("only the candidate's test delta");
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
			"rerun both focused commands sequentially without mutation",
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
			"When zero findings are retained",
			"Remove every temporary file and rejected candidate hunk",
			"Before publication",
			"mach12:code-architect",
		);
		expect(normalization).toContain("exactly the expected failures");
		expect(normalization).toContain("targeted edits only");
		expect(normalization).toContain("dirty paths consist only of normalized retained test files");
		expect(normalization).toContain("no production diff");
		expect(normalization).toContain("no temporary investigation files");
		expect(normalization).toMatch(/final node ID.+reproducible/);
		expect(normalization).toContain("minimal production fix proposals");
		expect(normalization).toContain("do not mutate");
		expect(normalization).toContain("require a clean primary worktree");
		expect(normalization).toContain("skip final-node and consolidated-red execution");
		expect(normalization).toContain("skip architect dispatch");
		expect(normalization).toContain("verified no-findings review artifact");
		expect(normalization).toContain("compare the complete diff and untracked contents byte-for-byte");
		expect(normalization).toContain("a path-only check is insufficient");
	});

	it("publishes a verified fix-compatible review artifact before the fresh forced handoff", () => {
		const publication = section(command, "## Step 7:");
		expect(publication).toContain("`<!-- mach12-review -->` as the first line");
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
		expect(publication).toContain("consolidated red command and result");
		expect(publication).toContain("reviewed head and merge-base identities");
		expect(publication).toContain("only normalized test changes remain");
		expect(publication).toContain("/mach12:gh-comment pr <pr-number>");
		expect(publication).toContain("numeric comment ID");
		expect(publication).toContain("complete body exactly equals the prepared body");
		expect(publication).toContain(
			"search existing PR comments for one whose complete body exactly equals the prepared body",
		);
		expect(publication).toContain("marker and head identity alone are insufficient");
		expect(publication).toMatch(/never blindly retry/i);
		expect(publication).toContain("`none — zero retained findings` result");
		expect(publication).toContain("include exactly one selector-visible context entry");
		expect(publication).toContain(
			"`message`: `/mach12:pr-validation-assessment <pr-number> --review-comment <numeric-comment-id>`",
		);
		expect(publication).toContain("`fresh_session`: `true`");
		expect(publication).toContain(
			'`reason`: "Independently reassess the retained executable proofs in a fresh session."',
		);
		expect(publication).toContain("Set `recommended_next_step` to `0`");
		expectInOrder(publication, "Prepare", "Post", "Verify", "After delivering your answer");
	});
});
