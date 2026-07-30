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
