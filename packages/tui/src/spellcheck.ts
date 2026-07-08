export interface SpellcheckRange {
	start: number;
	end: number;
}

export interface SpellcheckProvider {
	getMisspelledRanges(lineIndex: number): SpellcheckRange[];
	textChanged(lines: string[]): void;
	onUpdate: (() => void) | null;
}
