import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";

const commandPath = join(__dirname, "../scramjet/commands/scramjet:troubleshoot.md");
const command = readFileSync(commandPath, "utf8");
const parsed = parseCommandFile(commandPath, command, "scramjet");

function body(): string {
	if (!parsed.ok) throw new Error(parsed.error);
	return parsed.def.body;
}

describe("scramjet:troubleshoot", () => {
	it("is a read-oriented top-level command with open issue routing", () => {
		expect(parsed.ok).toBe(true);
		if (!parsed.ok) return;
		expect(parsed.def.allowedTools).toEqual(["read", "bash", "grep", "glob"]);
		expect(parsed.def.delegateOnly).toBeUndefined();
		expect(parsed.def.next).toEqual({
			mode: "open",
			candidates: [
				{
					name: "mach12:issue-create",
					hint: expect.stringContaining("reviewable issue draft"),
				},
			],
		});
	});

	it("requires exactly the five concise visible sections", () => {
		const headings = body().match(/^## .+$/gm) ?? [];
		expect(headings).toEqual([
			"## User intent",
			"## What actually occurred",
			"## Root cause analysis",
			"## What should have occurred",
			"## Recommended next steps",
		]);
		expect(body()).toContain("exactly these five headings");
		expect(body()).not.toMatch(/handoff|evidence inventory|provenance taxonomy|redaction report|artifact append/i);
	});

	it("covers all six internal analysis lenses without adding visible reports", () => {
		for (const lens of [
			"agent interpretation",
			"command instructions",
			"harness and tool design",
			"user input",
			"historical recurrence",
			"user experience",
		]) {
			expect(body()).toContain(lens);
		}
		expect(body()).toContain("internal lenses");
	});

	it("defines safe same-CWD historical lookup outcomes", () => {
		expect(body()).toContain("Current session journal");
		expect(body()).toContain("command-status summaries first");
		expect(body()).toContain("untrusted evidence");
		for (const outcome of ["Relevant match", "No match", "Unavailable", "Ambiguous"]) {
			expect(body()).toContain(outcome);
		}
		expect(body()).toContain("Do not guess another storage root");
		expect(body()).toContain("does not prove the symptom never occurred");
	});

	it("routes only verified continuations and protects off-machine publication", () => {
		expect(body()).toContain("registered top-level command");
		expect(body()).toContain("verified arguments");
		expect(body()).toContain("Never guess missing or sensitive arguments");
		expect(body()).toContain("fresh_session: false");
		expect(body()).toContain("review and redact");
		expect(body()).toMatch(/Do not .*publish a GitHub issue/);
		expect(body()).toContain("Do not edit command or source files");
		expect(body()).toContain("Do not put evidence, journal paths, tokens, or private values in selector messages");
	});
});
