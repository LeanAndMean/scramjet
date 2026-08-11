import type { ForgeTextEdit } from "./types.js";

export function controlSafeText(value: string): string {
	let output = "";
	for (let index = 0; index < value.length; index++) {
		const codeUnit = value.charCodeAt(index);
		if (codeUnit === 0x0a) {
			output += "\n";
			continue;
		}
		if (codeUnit === 0x09) {
			output += "\\t";
			continue;
		}
		if (codeUnit === 0x0d) {
			output += "\\r";
			continue;
		}
		if (
			codeUnit < 0x20 ||
			(codeUnit >= 0x7f && codeUnit <= 0x9f) ||
			codeUnit === 0x061c ||
			codeUnit === 0x200e ||
			codeUnit === 0x200f ||
			(codeUnit >= 0x202a && codeUnit <= 0x202e) ||
			(codeUnit >= 0x2066 && codeUnit <= 0x2069) ||
			codeUnit === 0xfffe ||
			codeUnit === 0xffff ||
			(codeUnit >= 0xd800 &&
				codeUnit <= 0xdfff &&
				!(codeUnit <= 0xdbff && value.charCodeAt(index + 1) >= 0xdc00 && value.charCodeAt(index + 1) <= 0xdfff))
		) {
			output += `\\u${codeUnit.toString(16).toUpperCase().padStart(4, "0")}`;
			continue;
		}
		if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
			output += value.slice(index, index + 2);
			index++;
			continue;
		}
		output += value[index];
	}
	return output;
}

export function applyExactEdits(original: string, edits: readonly ForgeTextEdit[], label: string): string {
	if (edits.length === 0) throw new Error(`${label} requires at least one edit`);
	const matches = edits.map((edit, index) => {
		if (edit.oldText === "") throw new Error(`${label} edit ${index + 1} oldText must not be empty`);
		if (edit.oldText === edit.newText) throw new Error(`${label} edit ${index + 1} is a no-op`);
		const positions: number[] = [];
		let position = original.indexOf(edit.oldText);
		while (position !== -1) {
			positions.push(position);
			position = original.indexOf(edit.oldText, position + 1);
		}
		if (positions.length === 0) throw new Error(`${label} edit ${index + 1} oldText was not found exactly`);
		if (positions.length > 1) throw new Error(`${label} edit ${index + 1} oldText is not unique`);
		return { start: positions[0], end: positions[0] + edit.oldText.length, newText: edit.newText };
	});

	const ordered = [...matches].sort((left, right) => left.start - right.start);
	for (let index = 1; index < ordered.length; index++) {
		if (ordered[index].start < ordered[index - 1].end) throw new Error(`${label} edits overlap`);
	}
	let result = original;
	for (const match of ordered.reverse()) {
		result = result.slice(0, match.start) + match.newText + result.slice(match.end);
	}
	return result;
}
