import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { loadSkillsFromDir, parseFrontmatter } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
import { parseCommandFile } from "../src/commands/loader.js";

const commandPath = join(__dirname, "../scramjet/commands/scramjet:troubleshoot.md");
const command = readFileSync(commandPath, "utf8");
const parsed = parseCommandFile(commandPath, command, "scramjet");

function body(): string {
	if (!parsed.ok) throw new Error(parsed.error);
	return parsed.def.body;
}

const specialistNames = [
	"scramjet:command-architect",
	"scramjet:command-failure-analyst",
	"scramjet:command-reviewer",
	"scramjet:command-set-explorer",
	"scramjet:independent-command-assessor",
	"scramjet:instruction-semantics-analyzer",
	"scramjet:structural-mapper",
];

const specialistsPath = join(__dirname, "../scramjet/agents");

describe("Scramjet command specialists", () => {
	it("ships the exact parseable specialist roster with matching filenames and distinct descriptions", () => {
		const files = readdirSync(specialistsPath)
			.filter((file) => file.endsWith(".md"))
			.sort();
		expect(files).toEqual(specialistNames.map((name) => `${name}.md`));

		const definitions = files.map((file) => {
			const parsedAgent = parseFrontmatter<Record<string, unknown>>(
				readFileSync(join(specialistsPath, file), "utf8"),
			);
			return { file, frontmatter: parsedAgent.frontmatter, body: parsedAgent.body.trim() };
		});
		const descriptions = definitions.map(({ frontmatter }) => frontmatter.description);

		expect(definitions.map(({ file, frontmatter }) => `${frontmatter.name}.md` === file)).toEqual(
			Array(7).fill(true),
		);
		expect(descriptions.every((description) => typeof description === "string" && description.trim())).toBe(true);
		expect(definitions.every(({ body }) => body)).toBe(true);
		expect(
			new Set(
				descriptions
					.filter((description): description is string => typeof description === "string")
					.map((description) => description.trim()),
			).size,
		).toBe(7);
	});

	it("defines the structural mapper's bounded read-only evidence contract", () => {
		const source = readFileSync(join(specialistsPath, "scramjet:structural-mapper.md"), "utf8");
		const mapper = parseFrontmatter<Record<string, unknown>>(source);

		expect(mapper.frontmatter).toMatchObject({
			name: "scramjet:structural-mapper",
			description:
				"Produces bounded current-state evidence about responsibilities, dependencies, contracts, consumers, and evidence limits.",
			tools: "read, grep, find, ls",
		});
		for (const section of [
			"Task boundary and authority",
			"System map",
			"Module ownership",
			"Contract baseline",
			"Evidence limits",
		]) {
			expect(mapper.body).toContain(section);
		}
		expect(mapper.body).toContain("Verify material supplied claims and documentation against current source");
		expect(mapper.body).toContain("unknowable external consumers");
		expect(mapper.body).toContain("do not design a replacement architecture");
		expect(mapper.body).toContain("Do not mutate, execute project tools, publish, delegate, interact with the user");
	});
});

describe("Scramjet command authoring skill", () => {
	it("loads the packaged writing-scramjet-commands skill", () => {
		const result = loadSkillsFromDir({ dir: join(__dirname, "../skills"), source: "package" });

		expect(result.diagnostics).toEqual([]);
		expect(result.skills).toHaveLength(1);
		expect(result.skills[0]?.name).toBe("writing-scramjet-commands");
		expect(readFileSync(result.skills[0]!.filePath, "utf8")).toContain("A Scramjet command is a generalized plan");
	});
});

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

	it("requires exactly the five concise visible answer sections", () => {
		const headings = body().match(/^## .+$/gm) ?? [];
		expect(headings[0]).toBe("## Goals");
		expect(headings.slice(1)).toEqual([
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
