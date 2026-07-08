import { describe, expect, it } from "vitest";
import { applySpellcheckDecoration } from "../src/components/editor.js";

const colorize = (s: string) => `\x1b[31m${s}\x1b[39m`;

describe("applySpellcheckDecoration", () => {
	it("returns text unchanged when no ranges provided", () => {
		expect(applySpellcheckDecoration("hello world", 0, 11, [], colorize)).toBe("hello world");
	});

	it("decorates a misspelled word range", () => {
		const result = applySpellcheckDecoration("hello wrold", 0, 11, [{ start: 6, end: 11 }], colorize);
		expect(result).toBe("hello \x1b[4:3m\x1b[31mwrold\x1b[39m\x1b[24m");
	});

	it("decorates multiple separate ranges independently", () => {
		const result = applySpellcheckDecoration(
			"teh wrold tset",
			0,
			14,
			[
				{ start: 0, end: 3 },
				{ start: 4, end: 9 },
				{ start: 10, end: 14 },
			],
			colorize,
		);
		expect(result).toBe(
			"\x1b[4:3m\x1b[31mteh\x1b[39m\x1b[24m \x1b[4:3m\x1b[31mwrold\x1b[39m\x1b[24m \x1b[4:3m\x1b[31mtset\x1b[39m\x1b[24m",
		);
	});

	it("clips ranges to the chunk boundaries", () => {
		// Logical line: "hello wrold today" (0-17)
		// Chunk is "wrold tod" (chunkStart=6, chunkEnd=15)
		// Range covers "wrold" (6-11) — fully inside chunk
		const result = applySpellcheckDecoration("wrold tod", 6, 15, [{ start: 6, end: 11 }], colorize);
		expect(result).toBe("\x1b[4:3m\x1b[31mwrold\x1b[39m\x1b[24m tod");
	});

	it("clips a range that starts before the chunk", () => {
		// Range [4, 9) but chunk starts at 6
		const result = applySpellcheckDecoration("old today", 6, 15, [{ start: 4, end: 9 }], colorize);
		expect(result).toBe("\x1b[4:3m\x1b[31mold\x1b[39m\x1b[24m today");
	});

	it("clips a range that extends past the chunk", () => {
		// Range [6, 11) but chunk ends at 9
		const result = applySpellcheckDecoration("wro", 6, 9, [{ start: 6, end: 11 }], colorize);
		expect(result).toBe("\x1b[4:3m\x1b[31mwro\x1b[39m\x1b[24m");
	});

	it("ignores ranges entirely outside the chunk", () => {
		const result = applySpellcheckDecoration("good text", 0, 9, [{ start: 20, end: 25 }], colorize);
		expect(result).toBe("good text");
	});

	it("handles adjacent ranges without gap", () => {
		const result = applySpellcheckDecoration(
			"abcdef",
			0,
			6,
			[
				{ start: 0, end: 3 },
				{ start: 3, end: 6 },
			],
			colorize,
		);
		expect(result).toBe("\x1b[4:3m\x1b[31mabc\x1b[39m\x1b[24m\x1b[4:3m\x1b[31mdef\x1b[39m\x1b[24m");
	});

	it("handles empty text", () => {
		expect(applySpellcheckDecoration("", 0, 0, [], colorize)).toBe("");
	});

	it("handles range covering entire chunk", () => {
		const result = applySpellcheckDecoration("wrold", 0, 5, [{ start: 0, end: 5 }], colorize);
		expect(result).toBe("\x1b[4:3m\x1b[31mwrold\x1b[39m\x1b[24m");
	});
});
