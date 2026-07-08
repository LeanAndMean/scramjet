import type { SpellcheckProvider, SpellcheckRange } from "@leanandmean/tui";

interface Token {
	word: string;
	start: number;
	end: number;
}

const CODE_CHARS = new Set(["_", "/", "\\", ".", "@", "#", ":"]);

export function tokenizeLine(line: string): Token[] {
	const tokens: Token[] = [];
	const len = line.length;
	let i = 0;

	while (i < len) {
		// Skip non-word characters
		while (i < len && !isWordChar(line.charCodeAt(i))) i++;
		if (i >= len) break;

		// Collect word run: [a-zA-Z'] sequences
		const start = i;
		while (i < len && isWordChar(line.charCodeAt(i))) i++;

		// Trim leading/trailing apostrophes
		let wordStart = start;
		let wordEnd = i;
		while (wordStart < wordEnd && line[wordStart] === "'") wordStart++;
		while (wordEnd > wordStart && line[wordEnd - 1] === "'") wordEnd--;

		if (wordEnd - wordStart > 1) {
			tokens.push({ word: line.slice(wordStart, wordEnd), start: wordStart, end: wordEnd });
		}
	}

	return tokens;
}

function isWordChar(code: number): boolean {
	return (
		(code >= 65 && code <= 90) || // A-Z
		(code >= 97 && code <= 122) || // a-z
		code === 39 // '
	);
}

export function isCodeLikeToken(token: string): boolean {
	for (let i = 0; i < token.length; i++) {
		if (CODE_CHARS.has(token[i])) return true;
	}
	return false;
}

export function hasCodeLikeCasing(word: string): boolean {
	if (word === word.toUpperCase()) return false;
	if (word === word.toLowerCase()) return false;
	// Title case: only first letter uppercase
	if (word[0] === word[0].toUpperCase() && word.slice(1) === word.slice(1).toLowerCase()) return false;
	// Interior uppercase = code-like
	return true;
}

export class NspellProvider implements SpellcheckProvider {
	onUpdate: (() => void) | null = null;
	readonly ready: Promise<void>;

	private checker: { correct(word: string): boolean } | null = null;
	private cache = new Map<string, SpellcheckRange[]>();
	private currentLines: string[] = [];
	private debounceTimer: ReturnType<typeof setTimeout> | null = null;

	constructor() {
		this.ready = this.initDictionary();
	}

	private async initDictionary(): Promise<void> {
		try {
			const [nspellMod, { default: dictionary }] = await Promise.all([import("nspell"), import("dictionary-en")]);
			this.checker = nspellMod.default(dictionary.aff as Buffer, dictionary.dic as Buffer);
			// Re-check with dictionary now available
			if (this.currentLines.length > 0) {
				this.recheck();
			}
		} catch {
			// Dictionary load failure is non-fatal — spellcheck simply won't activate
		}
	}

	getMisspelledRanges(lineIndex: number): SpellcheckRange[] {
		if (!this.checker || lineIndex >= this.currentLines.length) return [];
		const line = this.currentLines[lineIndex];
		const cached = this.cache.get(line);
		if (cached !== undefined) return cached;
		const ranges = this.computeRanges(line);
		this.cache.set(line, ranges);
		return ranges;
	}

	textChanged(lines: string[]): void {
		this.currentLines = lines;
		if (this.debounceTimer !== null) {
			clearTimeout(this.debounceTimer);
		}
		this.debounceTimer = setTimeout(() => {
			this.debounceTimer = null;
			this.recheck();
		}, 150);
	}

	private recheck(): void {
		if (!this.checker) return;
		let changed = false;
		for (const line of this.currentLines) {
			if (!this.cache.has(line)) {
				this.cache.set(line, this.computeRanges(line));
				changed = true;
			}
		}
		if (changed && this.onUpdate) {
			this.onUpdate();
		}
	}

	private computeRanges(line: string): SpellcheckRange[] {
		if (!this.checker) return [];
		const ranges: SpellcheckRange[] = [];
		const words = line.split(/\s+/);
		let offset = 0;

		for (const wholeToken of words) {
			const tokenStart = line.indexOf(wholeToken, offset);
			offset = tokenStart + wholeToken.length;

			const stripped = wholeToken.replace(/[.,!?;:)\]]+$/, "");
			if (stripped.length > 0 && isCodeLikeToken(stripped)) continue;

			const tokens = tokenizeLine(wholeToken);
			for (const token of tokens) {
				if (hasCodeLikeCasing(token.word)) continue;
				if (!this.checker.correct(token.word)) {
					ranges.push({ start: tokenStart + token.start, end: tokenStart + token.end });
				}
			}
		}

		return ranges;
	}
}
