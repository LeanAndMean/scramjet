import { describe, expect, it } from "vitest";
import { parseScramjetCommandBlock } from "../src/core/scramjet-command-parser.js";

describe("parseScramjetCommandBlock", () => {
	it("parses a typical command with user-context", () => {
		const text = `<scramjet-command name="mach12:issue-plan">
# Issue Plan

<user-context>
82
</user-context>

## Step 1: Parse Input
</scramjet-command>`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.name).toBe("mach12:issue-plan");
		expect(result!.content).toContain("# Issue Plan");
		expect(result!.content).toContain("## Step 1: Parse Input");
		expect(result!.userContext).toBe("82");
		expect(result!.userMessage).toBeUndefined();
	});

	it("parses a command without user-context", () => {
		const text = `<scramjet-command name="mach12:push">
# Push Changes

Commit and push the current branch.
</scramjet-command>`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.name).toBe("mach12:push");
		expect(result!.content).toContain("# Push Changes");
		expect(result!.userContext).toBeUndefined();
		expect(result!.userMessage).toBeUndefined();
	});

	it("returns null for non-matching text", () => {
		expect(parseScramjetCommandBlock("Hello world")).toBeNull();
		expect(parseScramjetCommandBlock('<skill name="foo" location="bar">\ncontent\n</skill>')).toBeNull();
		expect(parseScramjetCommandBlock('<scramjet-command name="test">')).toBeNull();
		expect(parseScramjetCommandBlock("")).toBeNull();
	});

	it("handles complex multi-line user-context with XML", () => {
		const text = `<scramjet-command name="mach12:issue-create">
# Create Issue

<user-context>
Add a feature that handles <xml> tags
and multiple lines of context

- with lists
- and **markdown**
</user-context>

## Step 1
</scramjet-command>`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.name).toBe("mach12:issue-create");
		expect(result!.userContext).toBe(
			"Add a feature that handles <xml> tags\nand multiple lines of context\n\n- with lists\n- and **markdown**",
		);
	});

	it("extracts userMessage after closing tag", () => {
		const text = `<scramjet-command name="mach12:issue-plan">
# Plan

<user-context>
55
</user-context>
</scramjet-command>

Here is some additional context the user typed after the command.`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.name).toBe("mach12:issue-plan");
		expect(result!.userContext).toBe("55");
		expect(result!.userMessage).toBe("Here is some additional context the user typed after the command.");
	});

	it("returns userContext undefined for empty user-context tags", () => {
		const text = `<scramjet-command name="mach12:push">
# Push

<user-context>
</user-context>
</scramjet-command>`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.userContext).toBeUndefined();
	});

	it("handles caller-context (subroutine pattern) without extracting as userContext", () => {
		const text = `<scramjet-command name="mach12:gh-issue-read">
# Read Issue

<caller-context>
82 --marker mach12-plan
</caller-context>

## Step 1
</scramjet-command>`;

		const result = parseScramjetCommandBlock(text);
		expect(result).not.toBeNull();
		expect(result!.name).toBe("mach12:gh-issue-read");
		expect(result!.userContext).toBeUndefined();
		expect(result!.content).toContain("<caller-context>");
	});
});
