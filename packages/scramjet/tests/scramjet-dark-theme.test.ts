import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { getCurrentThemeName, loadThemeFromPath, resolveThemeName, type ThemeColor } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";

const themePath = join(dirname(fileURLToPath(import.meta.url)), "..", "themes", "scramjet-dark.json");

const FG_TOKENS: ThemeColor[] = [
	"accent",
	"border",
	"borderAccent",
	"borderMuted",
	"success",
	"error",
	"warning",
	"muted",
	"dim",
	"text",
	"thinkingText",
	"userMessageText",
	"customMessageText",
	"customMessageLabel",
	"toolTitle",
	"toolOutput",
	"mdHeading",
	"mdLink",
	"mdLinkUrl",
	"mdCode",
	"mdCodeBlock",
	"mdCodeBlockBorder",
	"mdQuote",
	"mdQuoteBorder",
	"mdHr",
	"mdListBullet",
	"toolDiffAdded",
	"toolDiffRemoved",
	"toolDiffContext",
	"syntaxComment",
	"syntaxKeyword",
	"syntaxFunction",
	"syntaxVariable",
	"syntaxString",
	"syntaxNumber",
	"syntaxType",
	"syntaxOperator",
	"syntaxPunctuation",
	"thinkingOff",
	"thinkingMinimal",
	"thinkingLow",
	"thinkingMedium",
	"thinkingHigh",
	"thinkingXhigh",
	"bashMode",
	"spellcheckError",
];

const BG_TOKENS = [
	"selectedBg",
	"userMessageBg",
	"customMessageBg",
	"toolPendingBg",
	"toolSuccessBg",
	"toolErrorBg",
] as const;

describe("scramjet-dark theme file", () => {
	it("loads via loadThemeFromPath without throwing (F3)", () => {
		expect(() => loadThemeFromPath(themePath, "truecolor")).not.toThrow();
	});

	it("declares name === scramjet-dark", () => {
		const theme = loadThemeFromPath(themePath, "truecolor");
		expect(theme.name).toBe("scramjet-dark");
	});

	it("resolves all 52 color tokens at runtime, including spellcheckError", () => {
		const theme = loadThemeFromPath(themePath, "truecolor");
		expect(FG_TOKENS.length + BG_TOKENS.length).toBe(52);
		for (const token of FG_TOKENS) {
			expect(() => theme.fg(token, "x"), `fg token ${token} should resolve`).not.toThrow();
		}
		for (const token of BG_TOKENS) {
			expect(() => theme.bg(token, "x"), `bg token ${token} should resolve`).not.toThrow();
		}
		expect(() => theme.fg("spellcheckError", "x")).not.toThrow();
	});

	it("includes the export section with pageBg, cardBg, infoBg", () => {
		const raw = JSON.parse(readFileSync(themePath, "utf-8")) as {
			export?: Record<string, string>;
			colors: Record<string, string>;
		};
		expect(raw.export).toBeDefined();
		expect(raw.export).toHaveProperty("pageBg");
		expect(raw.export).toHaveProperty("cardBg");
		expect(raw.export).toHaveProperty("infoBg");
	});

	it("defines exactly 52 color tokens in the JSON", () => {
		const raw = JSON.parse(readFileSync(themePath, "utf-8")) as { colors: Record<string, string> };
		expect(Object.keys(raw.colors).sort()).toEqual([...FG_TOKENS, ...BG_TOKENS].sort());
	});
});

describe("coding-agent public theme API (F2)", () => {
	it("exports loadThemeFromPath, resolveThemeName, getCurrentThemeName", () => {
		expect(typeof loadThemeFromPath).toBe("function");
		expect(typeof resolveThemeName).toBe("function");
		expect(typeof getCurrentThemeName).toBe("function");
	});
});
