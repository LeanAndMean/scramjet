import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const SET_DIR = resolve(HERE, "..", "scramjet");
const COMMAND_PATH = resolve(SET_DIR, "commands", "scramjet:troubleshoot.md");

function commandSource(): string {
	return readFileSync(COMMAND_PATH, "utf-8");
}

function expectInOrder(source: string, values: string[]): void {
	let cursor = -1;
	for (const value of values) {
		const next = source.indexOf(value, cursor + 1);
		expect(next, `expected ${JSON.stringify(value)} after offset ${cursor}`).toBeGreaterThan(cursor);
		cursor = next;
	}
}

describe("scramjet troubleshooting command", () => {
	it("C1 ships a public unrestricted terminus with one user-context substitution", () => {
		const set = readFileSync(resolve(SET_DIR, "set.yaml"), "utf-8");
		const source = commandSource();
		const parsed = parseCommandFile(COMMAND_PATH, source, "scramjet");

		expect(set).toContain("name: scramjet");
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		expect(parsed.def.name).toBe("scramjet:troubleshoot");
		expect(parsed.def.description).toBeTruthy();
		expect(parsed.def.argumentHint).toBe("[focus or symptom]");
		expect(parsed.def.next).toBeUndefined();
		expect(parsed.def.delegateOnly).toBeUndefined();
		expect(parsed.def.allowedTools).toBeUndefined();
		expect(source.match(/\$ARGUMENTS/g)).toHaveLength(1);
		expect(source.match(/<user-context>/g)).toHaveLength(1);
		expect(source.match(/<\/user-context>/g)).toHaveLength(1);
		expect(source).toContain("<user-context>\n$ARGUMENTS\n</user-context>");
		expect(source).not.toContain("/mach12:");
		expect(source).not.toContain("subagent");
		expect(source).not.toContain("delegate");
	});

	it("C2 keeps target reconciliation and recovery ahead of diagnosis", () => {
		const source = commandSource();

		expectInOrder(source, [
			"get_scramjet_troubleshooting_evidence",
			"`open` → `select` → `index`/`read`",
			"Target Selection",
			"Recovery",
			"Diagnosis",
			"Smallest Proposed Improvement",
		]);
		for (const value of [
			"not-attempted",
			"confirmed-not-applied",
			"confirmed-applied",
			"partially-applied",
			"indeterminate",
			"not-needed",
			"recovered",
			"partially-recovered",
			"blocked",
			"declined",
			"withheld-unsafe",
		]) {
			expect(source).toContain(`\`${value}\``);
		}
		for (const value of [
			"command-defect",
			"missing-project-context",
			"prompt-adherence-failure",
			"harness-or-tool-failure",
			"external-or-transient-failure",
			"incorrect-workflow-abstraction",
			"indeterminate",
			"run-specific",
			"plausibly-general",
		]) {
			expect(source).toContain(`\`${value}\``);
		}
		expect(source).toContain("tool-issued `inv-v1-…` invocation reference");
		expect(source).toContain("nearest non-troubleshoot invocation");
		expect(source).toContain("multiple candidates plausibly match");
		expect(source).toContain("Never repeat an action already confirmed applied");
		expect(source).toContain("get_scramjet_user_input");
		expectInOrder(source, [
			"Present the exact proposed action, current evidence, consequence, and retry reason",
			"fresh informed approval immediately before the action",
			"Treat cancellation or No as no authorization",
			"Perform only the approved action",
			"Re-verify local and external state afterward",
		]);
		expect(source).toContain('`type: "confirm"`');
		expect(source).toContain("Do not add or use a bespoke recovery tool");
		expect(source).toContain("cross-session and sibling-branch retrieval are unsupported");
		expect(source).toContain("SNAPSHOT_NOT_FOUND");
		expect(source).toContain("SNAPSHOT_BRANCH_CHANGED");
		expect(source).toContain("Never reuse stale invocation or evidence references");
	});

	it("C3 pins the persisted handoff, redaction, and completion contract", () => {
		const source = commandSource();
		const headings = [
			"Handoff ID and Source Invocation/Session Reference",
			"Recovery",
			"Evidence Availability",
			"Observed Facts",
			"Interpreter Feedback",
			"Analysis and Classification",
			"Alternatives and Confidence Boundary",
			"Generalization",
			"Smallest Proposed Improvement",
			"Reproduction and Expected Graceful Recovery",
			"Disposition",
			"Evidence Gaps",
			"Redaction Notes",
		];
		expectInOrder(source, headings);
		expect(source.match(/<!-- scramjet-troubleshooting-handoff-v1 id=/g)).toHaveLength(1);
		expect(source).toContain('<!-- scramjet-troubleshooting-handoff-v1 id="sth-v1-…" -->');
		expect(source).toContain("The marker must appear exactly once");
		expect(source).toContain("Handoff ID:");
		for (const reference of [
			"transcript",
			"tool-call",
			"tool-result",
			"status",
			"log",
			"compaction",
			"source",
			"guide",
		]) {
			expect(source).toContain(`[${reference}:evd-v1-…]`);
		}
		for (const disposition of [
			"no-change",
			"operational-or-documentation-correction",
			"manual-command-authoring",
			"ordinary-issue-suggested",
		]) {
			expect(source).toContain(`\`${disposition}\``);
		}
		expect(source).toContain("Handoff <handoff-id>; recovery=<recovery-outcome>; classification=<primary-cause>.");
		expect(source).toContain("After delivering the complete answer");
		expect(source).toContain("report_scramjet_command_status");
		expect(source).toContain('`status: "completed"`');
		expect(source).toContain("no `next_steps`");
		expect(source).toContain("Do not edit command sources");
		expect(source).toContain("create issues");
		expect(source).toContain("write a handoff file");
		expect(source).toContain("publish");
		expect(source).toContain("raw journal, session, or tool-call IDs");
		for (const redactionRule of [
			"credentials, API keys, tokens, cookies, authorization and proxy headers",
			"passwords, and private keys",
			"personal identifiers",
			"internal hosts, private addresses, non-public URLs, tenant names",
			"internal repository identifiers",
			"repository absolute paths",
			"home paths",
			"irrelevant user content",
			"images, binary or base64 payloads, opaque details, hidden thinking, or thought signatures",
			"Paraphrase sensitive commands and tool payloads",
			"never removed originals",
		]) {
			expect(source).toContain(redactionRule);
		}
	});
});
