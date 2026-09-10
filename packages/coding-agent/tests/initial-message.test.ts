import { describe, expect, it } from "vitest";
import { parseArgs } from "../src/cli/args.js";
import { buildInitialMessage, normalizePipedStdinContent } from "../src/cli/initial-message.js";

describe("piped stdin initial message", () => {
	it("preserves the task prefix and internal newlines while trimming outer whitespace", () => {
		const stdinContent = normalizePipedStdinContent("\n  Task: first line\nsecond line  \n\n");
		const result = buildInitialMessage({ parsed: parseArgs([]), stdinContent });

		expect(result.initialMessage).toBe("Task: first line\nsecond line");
	});
});
