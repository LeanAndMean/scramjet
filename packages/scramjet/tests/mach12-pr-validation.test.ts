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

describe("mach12 pr-validation-assessment independent-proof workflow", () => {
	const command = readCommand("pr-validation-assessment").content;

	it("reacquires exact authoritative evidence before enforcing stale-state guards", () => {
		const handoff = section(command, "## Step 2:", "## Step 3:");
		expect(handoff).toContain("exact numeric review comment ID");
		expect(handoff).toContain("gh api repos/:owner/:repo/issues/comments/<review-comment-id>");
		expect(handoff).toMatch(/do not use heuristic marker discovery/i);
		for (const source of [
			"PR title, body, base and head identities, files, commits, and complete comments",
			"linked issue",
			"<!-- mach12-plan -->",
			"complete merge-base-to-head diff",
			"tests adjacent to every changed production boundary",
			"prior review, assessment, decision, and fix artifacts",
		]) {
			expect(handoff).toContain(source);
		}
		for (const field of [
			"reviewed head OID",
			"actual merge-base OID",
			"retained test paths and node IDs",
			"expected dirty-path set",
			"candidate dispositions",
			"scope and practical-impact claims",
		]) {
			expect(handoff).toContain(field);
		}

		const guards = section(command, "## Step 3:", "## Step 4:");
		expect(guards).toContain("local and GitHub heads exactly equal the reviewed head OID");
		expect(guards).toContain("empty index");
		expect(guards).toContain("dirty paths exactly equal the expected normalized test paths");
		expect(guards).toContain("no production changes");
		expect(guards).toContain("no temporary investigation files");
		expect(guards).toMatch(/each recorded node ID is discoverable/i);
		expect(guards).toContain(
			"without cleaning, resetting, stashing, adapting proofs to a new head, or routing to fixes",
		);
		expectInOrder(command, "## Step 2:", "## Step 3:", "## Step 4:", "rerun every retained node");
	});

	it("keeps the main agent neutral and independently re-derives every proof claim", () => {
		const adjudication = section(command, "## Step 4:", "## Step 5:");
		expect(adjudication).toContain("Do not pre-classify");
		expect(adjudication).toContain("one holistic `mach12:independent-assessor`");
		expect(adjudication).toContain('agentScope: "user"');
		expect(adjudication).toContain("exact review body");
		expect(adjudication).toContain("authoritative context");
		expect(adjudication).toContain("rerun every retained node sequentially");
		expect(adjudication).toContain("consolidated command sequentially");
		for (const check of [
			"reproducibility and fixture realism",
			"intended contract and approved-plan scope",
			"merge-base classification",
			"claimed production-path sensitivity and root cause",
			"existing-coverage and redundancy status",
			"practical trigger, visible consequence, durable-state safety, frequency, and severity",
		]) {
			expect(adjudication).toContain(check);
		}
		for (const classification of [
			"genuine defect",
			"low-severity completion defect",
			"invalid proof",
			"intended behavior",
			"duplicate/already fixed",
			"pre-existing",
			"unresolved",
		]) {
			expect(adjudication).toContain(classification);
		}
		expect(adjudication).toContain("Do not use `Regression`");
	});

	it("removes rejected proofs before final verification and dispatches architects only for survivors", () => {
		const cleanup = section(command, "## Step 5:", "## Step 6:");
		expect(cleanup).toContain("targeted edits");
		for (const forbidden of ["repair", "weaken", "skip", "xfail", "rename", "relocate", "duplicate"]) {
			expect(cleanup).toContain(forbidden);
		}
		expectInOrder(
			cleanup,
			"Remove every second-pass rejected proof",
			"Rerun every surviving final node",
			"consolidated command",
			"verify the worktree",
			"mach12:code-architect",
		);
		expect(cleanup).toContain("If no findings survive, require a clean primary worktree");
		expect(cleanup).toContain("only for surviving root-cause clusters");
		expect(cleanup).toContain("minimum-sufficient production fixes");
		expect(cleanup).toContain("exact files and functions");
		expect(cleanup).toContain("preserved invariants");
		expect(cleanup).toContain("unchanged proof becomes green");
		expect(cleanup).toContain("cannot override the approved plan");
	});

	it("publishes and verifies a proof-preserving assessment artifact", () => {
		const artifact = section(command, "## Step 6:", "## Step 7:");
		expect(artifact).toContain("`<!-- mach12-assessment -->` as the first line");
		expect(artifact).toContain("exact review comment URL");
		for (const field of [
			"independent classification",
			"corrections to review claims",
			"final root-cause-to-node-ID mapping",
			"implementation constraints",
			"architect-informed staged repair plan",
			"rejected proofs and why they were removed",
		]) {
			expect(artifact).toContain(field);
		}
		expect(artifact).toContain("already in permanent behavioral suites");
		expect(artifact).toContain("must pass in place through production repairs");
		expect(artifact).toContain("must not be weakened, renamed, relocated, or duplicated");
		expect(artifact).toContain("/mach12:gh-comment pr <pr-number>");
		expect(artifact).toContain("numeric assessment comment ID");
		expect(artifact).toContain("complete body exactly equals the prepared body");
		expect(artifact).toMatch(/never blindly retry/i);
		expect(artifact).toContain("do not route onward");
		expectInOrder(artifact, "Prepare", "Post", "Verify");
	});

	it("emits complete fresh fix and pre-merge wires for each surviving outcome", () => {
		const routing = section(command, "## Step 7:");
		expect(routing).toContain("Route by the final independent classification");
		expect(routing).toContain("preserving each item's original F/S identifier");
		expect(routing).toContain("compact root-cause summaries");
		expect(routing).toContain("final node IDs");
		expect(routing).toContain("proof-preservation constraints");
		expect(routing).toContain("invalid, duplicate, pre-existing, unresolved, or rejected findings");
		expect(routing).toContain("review and assessment numeric comment IDs");

		const mixed = section(
			routing,
			"**When final classifications include both `genuine defect` and `low-severity completion defect`:**",
			"**When only final `genuine defect` classifications survive:**",
		);
		expect(mixed.match(/`message`:/g)).toHaveLength(3);
		expect(mixed.match(/`fresh_session`: `true`/g)).toHaveLength(3);
		expect(mixed.match(/`reason`:/g)).toHaveLength(3);
		expectInOrder(mixed, "<genuine-defect-ids>", "<genuine-and-low-severity-ids>", "/mach12:pr-pre-merge");
		expect(mixed).toContain("Set `recommended_next_step` to `0`, the genuine-only fix");

		const genuineOnly = section(
			routing,
			"**When only final `genuine defect` classifications survive:**",
			"**When only final `low-severity completion defect` classifications survive:**",
		);
		expect(genuineOnly.match(/`message`:/g)).toHaveLength(2);
		expect(genuineOnly.match(/`fresh_session`: `true`/g)).toHaveLength(2);
		expect(genuineOnly.match(/`reason`:/g)).toHaveLength(2);
		expectInOrder(genuineOnly, "<genuine-defect-ids>", "/mach12:pr-pre-merge");
		expect(genuineOnly).toContain("Set `recommended_next_step` to `0`, the fix pass");

		const optionalOnly = section(
			routing,
			"**When only final `low-severity completion defect` classifications survive:**",
			"**When no genuine or low-severity finding survives:**",
		);
		expect(optionalOnly.match(/`message`:/g)).toHaveLength(2);
		expect(optionalOnly.match(/`fresh_session`: `true`/g)).toHaveLength(2);
		expect(optionalOnly.match(/`reason`:/g)).toHaveLength(2);
		expectInOrder(optionalOnly, "/mach12:pr-pre-merge", "<low-severity-ids>");
		expect(optionalOnly).toContain("Set `recommended_next_step` to `0`, pre-merge");

		const none = section(routing, "**When no genuine or low-severity finding survives:**", "If publication");
		expect(none.match(/`message`:/g)).toHaveLength(1);
		expect(none.match(/`fresh_session`: `true`/g)).toHaveLength(1);
		expect(none.match(/`reason`:/g)).toHaveLength(1);
		expect(none).toContain("/mach12:pr-pre-merge <pr-number>");
		expect(none).not.toContain("/mach12:pr-review-fix");
		expect(none).toContain("Set `recommended_next_step` to `0`");
		expect(routing).toContain("If publication is partial or uncertain");
	});
});
