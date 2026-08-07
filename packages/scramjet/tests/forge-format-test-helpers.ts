import { expect } from "vitest";

export interface ForgeFormatNode {
	name: string;
	attributes: Record<string, string>;
	children: ForgeFormatNode[];
	text: string;
}

const SCALAR_NAMES = new Set([
	"label",
	"title",
	"body",
	"path",
	"previous-path",
	"status",
	"sha",
	"author",
	"created-at",
	"url",
	"id",
	"name",
	"conclusion",
]);

export function decodeForgeScalar(value: string): string {
	let output = "";
	for (let index = 0; index < value.length; ) {
		if (value[index] !== "^") {
			output += value[index++];
			continue;
		}
		const match = /^\^!([0-9A-F]{4});/.exec(value.slice(index));
		if (match === null) throw new Error("raw or malformed caret in scalar");
		output += String.fromCharCode(Number.parseInt(match[1], 16));
		index += match[0].length;
	}
	return output;
}

function directiveEnd(text: string, start: number): { end: number; selfClosing: boolean } {
	let quoted = false;
	for (let index = start + 1; index < text.length; index++) {
		if (text.startsWith("^!", index)) {
			const encoded = /^\^![0-9A-F]{4};/.exec(text.slice(index));
			if (encoded === null) throw new Error("malformed attribute escape");
			index += encoded[0].length - 1;
			continue;
		}
		if (text[index] === '"') quoted = !quoted;
		if (!quoted && (text[index] === "{" || text[index] === ";")) {
			return { end: index, selfClosing: text[index] === ";" };
		}
	}
	throw new Error("unterminated directive");
}

function parseAttributes(source: string): Record<string, string> {
	const attributes: Record<string, string> = {};
	let offset = 0;
	while (offset < source.length) {
		const match = /^ ([a-z][a-z0-9-]*)="((?:\^![0-9A-F]{4};|[^"])*)"/.exec(source.slice(offset));
		if (match === null) throw new Error(`invalid attributes: ${source.slice(offset)}`);
		if (match[1] in attributes) throw new Error(`duplicate attribute ${match[1]}`);
		attributes[match[1]] = decodeForgeScalar(match[2]);
		offset += match[0].length;
	}
	return attributes;
}

export function parseForgeDocument(text: string): ForgeFormatNode {
	const roots: ForgeFormatNode[] = [];
	const stack: ForgeFormatNode[] = [];
	let index = 0;
	while (index < text.length) {
		const opening = text.indexOf("^", index);
		const segment = text.slice(index, opening === -1 ? text.length : opening);
		const current = stack.at(-1);
		if (current !== undefined && SCALAR_NAMES.has(current.name)) current.text += decodeForgeScalar(segment);
		else if (segment.trim() !== "") throw new Error(`text outside scalar: ${segment}`);
		if (opening === -1) break;

		const encoded = /^\^![0-9A-F]{4};/.exec(text.slice(opening));
		if (encoded !== null) {
			if (current === undefined || !SCALAR_NAMES.has(current.name)) throw new Error("escape outside scalar");
			current.text += decodeForgeScalar(encoded[0]);
			index = opening + encoded[0].length;
			continue;
		}

		const close = /^\^([a-z][a-z0-9-]*)}/.exec(text.slice(opening));
		if (close !== null) {
			const closed = stack.pop();
			if (closed?.name !== close[1]) throw new Error(`mismatched close ${close[1]}`);
			index = opening + close[0].length;
			continue;
		}

		const nameMatch = /^\^([a-z][a-z0-9-]*)/.exec(text.slice(opening));
		if (nameMatch === null) throw new Error("invalid directive");
		const { end, selfClosing } = directiveEnd(text, opening);
		const node: ForgeFormatNode = {
			name: nameMatch[1],
			attributes: parseAttributes(text.slice(opening + nameMatch[0].length, end)),
			children: [],
			text: "",
		};
		const parent = stack.at(-1);
		if (parent === undefined) roots.push(node);
		else parent.children.push(node);
		if (!selfClosing) stack.push(node);
		index = end + 1;
	}
	if (stack.length !== 0) throw new Error(`unclosed directive ${stack.at(-1)?.name}`);
	if (roots.length !== 1 || roots[0].name !== "artifact") throw new Error("expected one artifact root");
	return roots[0];
}

export function scalarChildren(node: ForgeFormatNode, name: string): string[] {
	return node.children.filter((child) => child.name === name).map((child) => child.text);
}

export function child(node: ForgeFormatNode, name: string): ForgeFormatNode {
	const matches = node.children.filter((candidate) => candidate.name === name);
	expect(matches).toHaveLength(1);
	return matches[0];
}

export function canonicalWithoutContinuation(content: string): string {
	const marker = content.lastIndexOf("\n\n^continue ");
	return marker === -1 ? content : content.slice(0, marker);
}
