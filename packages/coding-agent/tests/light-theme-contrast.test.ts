import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, test } from "vitest";
import { loadThemeFromPath, relativeLuminance } from "../src/modes/interactive/theme/theme.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const LIGHT_THEME_PATH = path.resolve(__dirname, "../src/modes/interactive/theme/light.json");

// ============================================================================
// ANSI Decoding Helpers
// ============================================================================

const XTERM_BASIC_COLORS: [number, number, number][] = [
	[0, 0, 0],
	[128, 0, 0],
	[0, 128, 0],
	[128, 128, 0],
	[0, 0, 128],
	[128, 0, 128],
	[0, 128, 128],
	[192, 192, 192],
	[128, 128, 128],
	[255, 0, 0],
	[0, 255, 0],
	[255, 255, 0],
	[0, 0, 255],
	[255, 0, 255],
	[0, 255, 255],
	[255, 255, 255],
];

function xterm256ToRgb(index: number): { r: number; g: number; b: number } {
	if (index < 16) {
		const [r, g, b] = XTERM_BASIC_COLORS[index];
		return { r, g, b };
	}
	if (index < 232) {
		const ci = index - 16;
		const ri = Math.floor(ci / 36);
		const gi = Math.floor((ci % 36) / 6);
		const bi = ci % 6;
		return {
			r: ri === 0 ? 0 : 55 + ri * 40,
			g: gi === 0 ? 0 : 55 + gi * 40,
			b: bi === 0 ? 0 : 55 + bi * 40,
		};
	}
	const gray = 8 + (index - 232) * 10;
	return { r: gray, g: gray, b: gray };
}

interface Rgb {
	r: number;
	g: number;
	b: number;
}

function parseAnsiToRgb(ansi: string): Rgb | null {
	// Terminal default: \x1b[39m or \x1b[49m
	if (ansi === "\x1b[39m" || ansi === "\x1b[49m") return null;

	// Truecolor: \x1b[38;2;R;G;Bm or \x1b[48;2;R;G;Bm
	const truecolorMatch = ansi.match(/^\x1b\[(?:38|48);2;(\d+);(\d+);(\d+)m$/);
	if (truecolorMatch) {
		return {
			r: Number(truecolorMatch[1]),
			g: Number(truecolorMatch[2]),
			b: Number(truecolorMatch[3]),
		};
	}

	// 256-color: \x1b[38;5;Nm or \x1b[48;5;Nm
	const color256Match = ansi.match(/^\x1b\[(?:38|48);5;(\d+)m$/);
	if (color256Match) {
		return xterm256ToRgb(Number(color256Match[1]));
	}

	throw new Error(`Unrecognized ANSI sequence: ${JSON.stringify(ansi)}`);
}

// ============================================================================
// Contrast Computation
// ============================================================================

function luminanceFromRgb(rgb: Rgb): number {
	return relativeLuminance(rgb.r / 255, rgb.g / 255, rgb.b / 255);
}

function contrastRatio(l1: number, l2: number): number {
	const lighter = Math.max(l1, l2);
	const darker = Math.min(l1, l2);
	return (lighter + 0.05) / (darker + 0.05);
}

const WHITE_LUMINANCE = 1.0;

// ============================================================================
// Semantic Usage Matrix
// ============================================================================

// Foreground tokens that render readable text on the terminal canvas (white bg).
// Excludes empty-string terminal defaults and decorative-only borders.
const CANVAS_TEXT_TOKENS = [
	"accent",
	"muted",
	"dim",
	"thinkingText",
	"success",
	"error",
	"warning",
	"mdHeading",
	"mdLink",
	"mdLinkUrl",
	"mdCode",
	"mdCodeBlock",
	"mdQuote",
	"mdListBullet",
	"syntaxComment",
	"syntaxKeyword",
	"syntaxFunction",
	"syntaxVariable",
	"syntaxString",
	"syntaxNumber",
	"syntaxType",
	"syntaxOperator",
	"syntaxPunctuation",
	"bashMode",
	"spellcheckError",
] as const;

// Foreground tokens that render inside tool execution boxes.
const TOOL_BOX_TEXT_TOKENS = [
	"toolOutput",
	"accent",
	"error",
	"warning",
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
] as const;

// Tool background tokens
const TOOL_BACKGROUNDS = ["toolPendingBg", "toolSuccessBg", "toolErrorBg"] as const;

// Foreground tokens on customMessageBg (label + markdown + syntax in code blocks)
const CUSTOM_MSG_TEXT_TOKENS = [
	"customMessageLabel",
	"mdHeading",
	"mdLink",
	"mdLinkUrl",
	"mdCode",
	"mdCodeBlock",
	"mdQuote",
	"mdListBullet",
	"syntaxComment",
	"syntaxKeyword",
	"syntaxFunction",
	"syntaxVariable",
	"syntaxString",
	"syntaxNumber",
	"syntaxType",
	"syntaxOperator",
	"syntaxPunctuation",
] as const;

// Foreground tokens that render markdown inside user message boxes
const USER_MSG_MARKDOWN_TOKENS = [
	"mdHeading",
	"mdLink",
	"mdLinkUrl",
	"mdCode",
	"mdCodeBlock",
	"mdQuote",
	"mdListBullet",
	"syntaxComment",
	"syntaxKeyword",
	"syntaxFunction",
	"syntaxVariable",
	"syntaxString",
	"syntaxNumber",
	"syntaxType",
	"syntaxOperator",
	"syntaxPunctuation",
] as const;

// Foreground tokens on selectedBg
const SELECTED_TEXT_TOKENS = ["accent", "muted", "dim"] as const;

// All six element backgrounds that need visual separation from white
const ALL_ELEMENT_BACKGROUNDS = [
	"selectedBg",
	"userMessageBg",
	"customMessageBg",
	"toolPendingBg",
	"toolSuccessBg",
	"toolErrorBg",
] as const;

// Tokens that are empty-string (terminal defaults) — verified but not contrast-tested
const TERMINAL_DEFAULT_TOKENS = ["text", "userMessageText", "customMessageText", "toolTitle"] as const;

// ============================================================================
// Tests
// ============================================================================

type ColorMode = "truecolor" | "256color";
const MODES: ColorMode[] = ["truecolor", "256color"];

describe("light theme contrast contract", () => {
	const themes = Object.fromEntries(MODES.map((mode) => [mode, loadThemeFromPath(LIGHT_THEME_PATH, mode)])) as Record<
		ColorMode,
		ReturnType<typeof loadThemeFromPath>
	>;

	describe("empty-string tokens remain terminal defaults", () => {
		for (const mode of MODES) {
			test(`${mode}: terminal-default tokens emit reset sequences`, () => {
				const theme = themes[mode];
				for (const token of TERMINAL_DEFAULT_TOKENS) {
					const ansi = theme.getFgAnsi(token);
					expect(ansi, `${token} should emit fg-reset in ${mode}`).toBe("\x1b[39m");
				}
			});
		}
	});

	describe("foreground text on canvas (white) — WCAG 4.5:1", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				for (const token of CANVAS_TEXT_TOKENS) {
					test(`${token} on white`, () => {
						const theme = themes[mode];
						const ansi = theme.getFgAnsi(token);
						const rgb = parseAnsiToRgb(ansi);
						expect(rgb, `${token} should not be terminal default`).not.toBeNull();
						const fgLum = luminanceFromRgb(rgb!);
						const ratio = contrastRatio(WHITE_LUMINANCE, fgLum);
						expect(
							ratio,
							`${token} (${mode}): ${formatRgb(rgb!)} on white = ${ratio.toFixed(2)}:1, need ≥4.5:1`,
						).toBeGreaterThanOrEqual(4.5);
					});
				}
			});
		}
	});

	describe("foreground text on tool backgrounds — WCAG 4.5:1", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				for (const bgToken of TOOL_BACKGROUNDS) {
					describe(`on ${bgToken}`, () => {
						const theme = themes[mode];
						const bgAnsi = theme.getBgAnsi(bgToken);
						const bgRgb = parseAnsiToRgb(bgAnsi);
						expect(bgRgb, `${bgToken} should not be terminal default`).not.toBeNull();
						const bgLum = luminanceFromRgb(bgRgb!);

						for (const fgToken of TOOL_BOX_TEXT_TOKENS) {
							test(`${fgToken}`, () => {
								const fgAnsi = theme.getFgAnsi(fgToken);
								const fgRgb = parseAnsiToRgb(fgAnsi);
								expect(fgRgb, `${fgToken} should not be terminal default`).not.toBeNull();
								const fgLum = luminanceFromRgb(fgRgb!);
								const ratio = contrastRatio(bgLum, fgLum);
								expect(
									ratio,
									`${fgToken} (${mode}): ${formatRgb(fgRgb!)} on ${bgToken} ${formatRgb(bgRgb!)} = ${ratio.toFixed(2)}:1, need ≥4.5:1`,
								).toBeGreaterThanOrEqual(4.5);
							});
						}
					});
				}
			});
		}
	});

	describe("foreground text on customMessageBg — WCAG 4.5:1", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				const theme = themes[mode];
				const bgAnsi = theme.getBgAnsi("customMessageBg");
				const bgRgb = parseAnsiToRgb(bgAnsi);
				expect(bgRgb).not.toBeNull();
				const bgLum = luminanceFromRgb(bgRgb!);

				for (const fgToken of CUSTOM_MSG_TEXT_TOKENS) {
					test(`${fgToken} on customMessageBg`, () => {
						const fgAnsi = theme.getFgAnsi(fgToken);
						const fgRgb = parseAnsiToRgb(fgAnsi);
						expect(fgRgb).not.toBeNull();
						const fgLum = luminanceFromRgb(fgRgb!);
						const ratio = contrastRatio(bgLum, fgLum);
						expect(
							ratio,
							`${fgToken} (${mode}): ${formatRgb(fgRgb!)} on customMessageBg ${formatRgb(bgRgb!)} = ${ratio.toFixed(2)}:1, need ≥4.5:1`,
						).toBeGreaterThanOrEqual(4.5);
					});
				}
			});
		}
	});

	describe("foreground text on userMessageBg — WCAG 4.5:1", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				const theme = themes[mode];
				const bgAnsi = theme.getBgAnsi("userMessageBg");
				const bgRgb = parseAnsiToRgb(bgAnsi);
				expect(bgRgb).not.toBeNull();
				const bgLum = luminanceFromRgb(bgRgb!);

				for (const fgToken of USER_MSG_MARKDOWN_TOKENS) {
					test(`${fgToken} on userMessageBg`, () => {
						const fgAnsi = theme.getFgAnsi(fgToken);
						const fgRgb = parseAnsiToRgb(fgAnsi);
						expect(fgRgb).not.toBeNull();
						const fgLum = luminanceFromRgb(fgRgb!);
						const ratio = contrastRatio(bgLum, fgLum);
						expect(
							ratio,
							`${fgToken} (${mode}): ${formatRgb(fgRgb!)} on userMessageBg ${formatRgb(bgRgb!)} = ${ratio.toFixed(2)}:1, need ≥4.5:1`,
						).toBeGreaterThanOrEqual(4.5);
					});
				}
			});
		}
	});

	describe("foreground text on selectedBg — WCAG 4.5:1", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				const theme = themes[mode];
				const bgAnsi = theme.getBgAnsi("selectedBg");
				const bgRgb = parseAnsiToRgb(bgAnsi);
				expect(bgRgb).not.toBeNull();
				const bgLum = luminanceFromRgb(bgRgb!);

				for (const fgToken of SELECTED_TEXT_TOKENS) {
					test(`${fgToken} on selectedBg`, () => {
						const fgAnsi = theme.getFgAnsi(fgToken);
						const fgRgb = parseAnsiToRgb(fgAnsi);
						expect(fgRgb).not.toBeNull();
						const fgLum = luminanceFromRgb(fgRgb!);
						const ratio = contrastRatio(bgLum, fgLum);
						expect(
							ratio,
							`${fgToken} (${mode}): ${formatRgb(fgRgb!)} on selectedBg ${formatRgb(bgRgb!)} = ${ratio.toFixed(2)}:1, need ≥4.5:1`,
						).toBeGreaterThanOrEqual(4.5);
					});
				}
			});
		}
	});

	describe("element backgrounds vs white — ≥1.30:1 separation", () => {
		for (const mode of MODES) {
			describe(mode, () => {
				for (const bgToken of ALL_ELEMENT_BACKGROUNDS) {
					test(`${bgToken} against white`, () => {
						const theme = themes[mode];
						const bgAnsi = theme.getBgAnsi(bgToken);
						const bgRgb = parseAnsiToRgb(bgAnsi);
						expect(bgRgb, `${bgToken} should not be terminal default`).not.toBeNull();
						const bgLum = luminanceFromRgb(bgRgb!);
						const ratio = contrastRatio(WHITE_LUMINANCE, bgLum);
						expect(
							ratio,
							`${bgToken} (${mode}): ${formatRgb(bgRgb!)} vs white = ${ratio.toFixed(3)}:1, need ≥1.30:1`,
						).toBeGreaterThanOrEqual(1.3);
					});
				}
			});
		}
	});
});

// ============================================================================
// Formatting helper
// ============================================================================

function formatRgb(rgb: Rgb): string {
	return `rgb(${rgb.r},${rgb.g},${rgb.b})`;
}
