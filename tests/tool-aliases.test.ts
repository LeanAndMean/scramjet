import { describe, expect, it } from "vitest";
import { CLAUDE_CODE_TOOL_NAMES, mapClaudeToolNameToPi } from "../src/tool-aliases/mapping.ts";

describe("CLAUDE_CODE_TOOL_NAMES", () => {
	it("exposes the seven Claude Code tool names in stable order", () => {
		expect(CLAUDE_CODE_TOOL_NAMES).toEqual(["Read", "Bash", "Edit", "Write", "Grep", "Glob", "LS"]);
	});
});

describe("mapClaudeToolNameToPi", () => {
	it("maps every Claude Code name to a known Pi tool", () => {
		for (const name of CLAUDE_CODE_TOOL_NAMES) {
			expect(mapClaudeToolNameToPi(name)).toMatch(/^(read|bash|edit|write|grep|find|ls)$/);
		}
	});

	it("maps Glob to Pi's find tool", () => {
		expect(mapClaudeToolNameToPi("Glob")).toBe("find");
	});

	it("maps LS to Pi's ls tool", () => {
		expect(mapClaudeToolNameToPi("LS")).toBe("ls");
	});

	it("returns undefined for unknown names", () => {
		expect(mapClaudeToolNameToPi("Nonexistent")).toBeUndefined();
	});

	it("is case-sensitive: lowercase native names do not match", () => {
		expect(mapClaudeToolNameToPi("read")).toBeUndefined();
		expect(mapClaudeToolNameToPi("bash")).toBeUndefined();
	});
});
