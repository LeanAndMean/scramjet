import { beforeAll, describe, expect, it, vi } from "vitest";
import {
	hasCodeLikeCasing,
	isCodeLikeToken,
	NspellProvider,
	tokenizeLine,
} from "../src/modes/interactive/spellcheck.js";

describe("tokenizeLine", () => {
	it("extracts words with byte offsets", () => {
		const tokens = tokenizeLine("hello world");
		expect(tokens).toEqual([
			{ word: "hello", start: 0, end: 5 },
			{ word: "world", start: 6, end: 11 },
		]);
	});

	it("handles apostrophes within words", () => {
		const tokens = tokenizeLine("don't it's");
		expect(tokens).toEqual([
			{ word: "don't", start: 0, end: 5 },
			{ word: "it's", start: 6, end: 10 },
		]);
	});

	it("strips leading/trailing apostrophes", () => {
		const tokens = tokenizeLine("'hello' 'world");
		expect(tokens).toEqual([
			{ word: "hello", start: 1, end: 6 },
			{ word: "world", start: 9, end: 14 },
		]);
	});

	it("splits on non-alpha separators", () => {
		const tokens = tokenizeLine("one-two three/four five.six");
		expect(tokens).toEqual([
			{ word: "one", start: 0, end: 3 },
			{ word: "two", start: 4, end: 7 },
			{ word: "three", start: 8, end: 13 },
			{ word: "four", start: 14, end: 18 },
			{ word: "five", start: 19, end: 23 },
			{ word: "six", start: 24, end: 27 },
		]);
	});

	it("returns empty array for empty string", () => {
		expect(tokenizeLine("")).toEqual([]);
	});

	it("returns empty array for whitespace-only", () => {
		expect(tokenizeLine("   \t  ")).toEqual([]);
	});

	it("skips single characters", () => {
		const tokens = tokenizeLine("I a b hello c");
		expect(tokens).toEqual([{ word: "hello", start: 6, end: 11 }]);
	});

	it("handles numbers and mixed content", () => {
		const tokens = tokenizeLine("hello 123 world");
		expect(tokens).toEqual([
			{ word: "hello", start: 0, end: 5 },
			{ word: "world", start: 10, end: 15 },
		]);
	});
});

describe("isCodeLikeToken", () => {
	it("returns true for tokens with underscore", () => {
		expect(isCodeLikeToken("snake_case")).toBe(true);
	});

	it("returns true for tokens with forward slash", () => {
		expect(isCodeLikeToken("path/to")).toBe(true);
	});

	it("returns true for tokens with backslash", () => {
		expect(isCodeLikeToken("path\\to")).toBe(true);
	});

	it("returns true for tokens with dot", () => {
		expect(isCodeLikeToken("file.txt")).toBe(true);
	});

	it("returns true for tokens with @", () => {
		expect(isCodeLikeToken("@user")).toBe(true);
	});

	it("returns true for tokens with #", () => {
		expect(isCodeLikeToken("#channel")).toBe(true);
	});

	it("returns true for tokens with colon", () => {
		expect(isCodeLikeToken("key:value")).toBe(true);
	});

	it("returns true for slash commands", () => {
		expect(isCodeLikeToken("/command")).toBe(true);
	});

	it("returns false for plain words", () => {
		expect(isCodeLikeToken("hello")).toBe(false);
		expect(isCodeLikeToken("world")).toBe(false);
	});

	it("returns false for words with apostrophes", () => {
		expect(isCodeLikeToken("don't")).toBe(false);
	});

	it("returns false for hyphenated words", () => {
		expect(isCodeLikeToken("well-known")).toBe(false);
	});
});

describe("hasCodeLikeCasing", () => {
	it("returns true for camelCase", () => {
		expect(hasCodeLikeCasing("camelCase")).toBe(true);
	});

	it("returns true for PascalCase", () => {
		expect(hasCodeLikeCasing("PascalCase")).toBe(true);
	});

	it("returns false for all-lowercase", () => {
		expect(hasCodeLikeCasing("hello")).toBe(false);
	});

	it("returns false for all-uppercase", () => {
		expect(hasCodeLikeCasing("HELLO")).toBe(false);
	});

	it("returns false for Title case (only first letter uppercase)", () => {
		expect(hasCodeLikeCasing("Hello")).toBe(false);
	});

	it("returns true for interior uppercase", () => {
		expect(hasCodeLikeCasing("iPhone")).toBe(true);
	});

	it("returns false for short all-uppercase abbreviations (<=4 chars)", () => {
		expect(hasCodeLikeCasing("API")).toBe(false);
		expect(hasCodeLikeCasing("URL")).toBe(false);
		expect(hasCodeLikeCasing("HTTP")).toBe(false);
	});

	it("returns false for longer all-uppercase words", () => {
		expect(hasCodeLikeCasing("HELLO")).toBe(false);
	});
});

describe("NspellProvider", () => {
	let provider: NspellProvider;

	beforeAll(async () => {
		provider = new NspellProvider();
		await provider.ready;
	});

	it("returns empty ranges for correctly spelled words", () => {
		provider.textChanged(["hello world"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([]);
	});

	it("returns ranges for misspelled words", () => {
		provider.textChanged(["helo wrold"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([
			{ start: 0, end: 4 },
			{ start: 5, end: 10 },
		]);
	});

	it("excludes code-like tokens from checking", () => {
		provider.textChanged(["helo file.txt camelCase /path"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([{ start: 0, end: 4 }]);
	});

	it("returns empty ranges before dictionary loads", () => {
		const freshProvider = new NspellProvider();
		freshProvider.textChanged(["helo wrold"]);
		const ranges = freshProvider.getMisspelledRanges(0);
		expect(ranges).toEqual([]);
	});

	it("returns empty ranges for out-of-bounds line index", () => {
		provider.textChanged(["hello"]);
		expect(provider.getMisspelledRanges(5)).toEqual([]);
	});

	it("handles mixed correct and incorrect words", () => {
		provider.textChanged(["the quik brown fox"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([{ start: 4, end: 8 }]);
	});

	it("handles words with apostrophes", () => {
		provider.textChanged(["don't won't shouldn't"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([]);
	});

	it("skips all-uppercase abbreviations", () => {
		provider.textChanged(["the API and URL"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([]);
	});

	it("catches misspellings adjacent to sentence-ending punctuation", () => {
		provider.textChanged(["Fix the tset."]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([{ start: 8, end: 12 }]);
	});

	it("still excludes code-like tokens with interior dots", () => {
		provider.textChanged(["open file.txt now"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([]);
	});

	it("catches misspellings before colons", () => {
		provider.textChanged(["Check thsi: hello"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([{ start: 6, end: 10 }]);
	});

	it("returns correct positions for duplicate misspelled words", () => {
		provider.textChanged(["helo world helo"]);
		const ranges = provider.getMisspelledRanges(0);
		expect(ranges).toEqual([
			{ start: 0, end: 4 },
			{ start: 11, end: 15 },
		]);
	});

	it("returns independent ranges per line", () => {
		provider.textChanged(["helo world", "the quik fox"]);
		const line0 = provider.getMisspelledRanges(0);
		const line1 = provider.getMisspelledRanges(1);
		expect(line0).toEqual([{ start: 0, end: 4 }]);
		expect(line1).toEqual([{ start: 4, end: 8 }]);
	});

	describe("cache pruning", () => {
		it("prunes stale entries after text changes", async () => {
			vi.useFakeTimers();
			const p = new NspellProvider();
			await p.ready;

			p.textChanged(["hello world", "the quick fox"]);
			p.getMisspelledRanges(0);
			p.getMisspelledRanges(1);
			expect(p.cacheSize).toBe(2);

			p.textChanged(["goodbye"]);
			await vi.advanceTimersByTimeAsync(200);

			expect(p.cacheSize).toBe(1);

			vi.useRealTimers();
		});

		it("retains cached results for shared lines", async () => {
			vi.useFakeTimers();
			const p = new NspellProvider();
			await p.ready;

			p.textChanged(["hello world", "the quick fox"]);
			p.getMisspelledRanges(0);
			p.getMisspelledRanges(1);
			const originalRanges = p.getMisspelledRanges(0);

			p.textChanged(["hello world", "a new line"]);
			await vi.advanceTimersByTimeAsync(200);

			expect(p.cacheSize).toBe(2);
			expect(p.getMisspelledRanges(0)).toBe(originalRanges);

			vi.useRealTimers();
		});
	});
});
