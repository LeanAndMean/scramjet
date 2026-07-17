import { describe, expect, it } from "vitest";
import { type BuildSystemPromptOptions, buildSystemPromptSections } from "../src/core/system-prompt.js";

function baseOptions(overrides?: Partial<BuildSystemPromptOptions>): BuildSystemPromptOptions {
	return {
		cwd: "/home/user/project",
		...overrides,
	};
}

function volatileText(options: BuildSystemPromptOptions): string {
	const sections = buildSystemPromptSections(options);
	const volatile = sections.find((s) => s.id === "volatile");
	expect(volatile).toBeDefined();
	return volatile!.text;
}

describe("buildSystemPromptSections — volatile environment tail", () => {
	it("includes session journal when sessionFile is provided", () => {
		const text = volatileText(baseOptions({ sessionFile: "/home/user/.scramjet/agent/sessions/sess.jsonl" }));
		expect(text).toContain("Current session journal: /home/user/.scramjet/agent/sessions/sess.jsonl");
	});

	it("normalizes Windows-style backslashes in sessionFile", () => {
		const text = volatileText(
			baseOptions({
				cwd: "C:\\Users\\dev\\project",
				sessionFile: "C:\\Users\\dev\\.scramjet\\agent\\sessions\\sess.jsonl",
			}),
		);
		expect(text).toContain("Current working directory: C:/Users/dev/project");
		expect(text).toContain("Current session journal: C:/Users/dev/.scramjet/agent/sessions/sess.jsonl");
		expect(text).not.toContain("\\");
	});

	it("omits session journal line when sessionFile is undefined", () => {
		const text = volatileText(baseOptions());
		expect(text).not.toContain("session journal");
	});

	it("omits session journal line when sessionFile is explicitly undefined", () => {
		const text = volatileText(baseOptions({ sessionFile: undefined }));
		expect(text).not.toContain("session journal");
	});

	it("still includes the nonexistent allocated path (no existence check)", () => {
		const fakePath = "/tmp/nonexistent-path-348/sessions/does-not-exist.jsonl";
		const text = volatileText(baseOptions({ sessionFile: fakePath }));
		expect(text).toContain(`Current session journal: ${fakePath}`);
	});

	it("volatile section remains last with cacheRetention none", () => {
		const sections = buildSystemPromptSections(
			baseOptions({ sessionFile: "/home/user/.scramjet/agent/sessions/s.jsonl" }),
		);
		const last = sections[sections.length - 1];
		expect(last.id).toBe("volatile");
		expect(last.cacheRetention).toBe("none");
	});

	it("always includes date and cwd alongside session journal", () => {
		const text = volatileText(baseOptions({ sessionFile: "/some/path.jsonl" }));
		expect(text).toMatch(/Current date: \d{4}-\d{2}-\d{2}/);
		expect(text).toContain("Current working directory: /home/user/project");
		expect(text).toContain("Current session journal: /some/path.jsonl");
	});
});
