// SCRAMJET-DIVERGENCE: retained component rows and content-relative reading anchors.
import { diffArrays } from "diff";
import { isImageLine } from "./terminal-image.js";
import type { Component } from "./tui.js";
import { extractAnsiCode, getSegmenter, truncateToWidth, visibleWidth } from "./utils.js";

export interface ViewportBlock {
	component: Component;
	finalized?: boolean;
	/** Increment when a finalized component's presentation changes. */
	revision?: number;
}

export interface ViewportOptions {
	getBlocks(): readonly ViewportBlock[];
}

export interface ViewportState {
	offset: number;
	totalRows: number;
	height: number;
	followingTail: boolean;
}

interface RenderedBlock extends ViewportBlock {
	lines: string[];
	start: number;
	width: number;
	generation: number;
}

interface Anchor {
	component: Component;
	row: number;
	grapheme: number;
	screenRow: number;
}

interface ContentToken {
	text: string;
	row: number;
	grapheme: number;
}

function comparison(lines: string[]): ContentToken[] {
	const tokens: ContentToken[] = [];
	for (let row = 0; row < lines.length; row++) {
		if (isImageLine(lines[row])) continue;
		let text = "";
		for (let i = 0; i < lines[row].length; ) {
			const ansi = extractAnsiCode(lines[row], i);
			if (ansi) i += ansi.length;
			else text += lines[row][i++];
		}
		let grapheme = 0;
		for (const { segment } of getSegmenter().segment(text)) {
			// Whitespace is correspondence-only: wrapping and padding must not change the painted rows.
			if (segment.trim() && visibleWidth(segment) > 0) tokens.push({ text: segment, row, grapheme });
			grapheme++;
		}
	}
	return tokens;
}

function correspondence(
	old: string[],
	next: string[],
	position: number,
): { position: number; exact: boolean } | undefined {
	let oldIndex = 0;
	let newIndex = 0;
	let nearest: { position: number; exact: boolean } | undefined;
	let distance = Infinity;
	for (const change of diffArrays(old, next)) {
		if (!change.added && !change.removed) {
			const candidate = Math.max(oldIndex, Math.min(position, oldIndex + change.count - 1));
			const delta = Math.abs(candidate - position);
			// Prefer the following surviving content when both sides are equally near.
			if (delta <= distance) {
				nearest = { position: newIndex + candidate - oldIndex, exact: delta === 0 };
				distance = delta;
			}
		}
		if (!change.added) oldIndex += change.count;
		if (!change.removed) newIndex += change.count;
	}
	return nearest;
}

function mapAnchor(anchor: Anchor, old: RenderedBlock, next: RenderedBlock): Anchor | undefined {
	if (old.lines === next.lines) return anchor;
	const rows = correspondence(old.lines, next.lines, anchor.row);
	if (rows?.exact) return { ...anchor, row: rows.position };

	const oldTokens = comparison(old.lines);
	const newTokens = comparison(next.lines);
	if (oldTokens.length === 0 && newTokens.length === 0 && next.lines.length > 0) {
		return { ...anchor, row: Math.min(anchor.row, next.lines.length - 1), grapheme: 0 };
	}
	let index = oldTokens.findIndex((token) => token.row === anchor.row && token.grapheme >= anchor.grapheme);
	if (index === -1) index = oldTokens.findIndex((token) => token.row > anchor.row);
	if (index === -1) index = oldTokens.length - 1;
	if (index >= 0) {
		const mapped = correspondence(
			oldTokens.map((token) => token.text),
			newTokens.map((token) => token.text),
			index,
		);
		if (mapped && (mapped.exact || !rows)) {
			const token = newTokens[mapped.position];
			return { ...anchor, row: token.row, grapheme: token.grapheme };
		}
	}
	return rows ? { ...anchor, row: rows.position, grapheme: 0 } : undefined;
}

interface ImagePlacement {
	sequence: string;
	row: number;
	col: number;
}

// Only built-in placement envelopes provide enough geometry for atomic viewport painting.
function imagePlacement(line: string, row: number): (ImagePlacement & { rows: number; columns: number }) | undefined {
	const kitty = line.match(/\x1b_G([^;]+);[\s\S]*\x1b\\/);
	if (kitty) {
		const params = new Map(kitty[1].split(",").map((part) => part.split("=")) as [string, string][]);
		if (params.get("C") !== "1") return undefined;
		return {
			sequence: kitty[0],
			row,
			col: visibleWidth(line.slice(0, kitty.index)),
			rows: Number(params.get("r")),
			columns: Number(params.get("c")),
		};
	}
	const iterm = line.match(/(?:\x1b\[(\d+)A)?(\x1b\]1337;File=[^\x07]*\x07)/);
	if (iterm) {
		const up = Number(iterm[1] ?? 0);
		return {
			sequence: iterm[2],
			row: row - up,
			col: visibleWidth(line.slice(0, iterm.index)),
			rows: up + 1,
			columns: Number(iterm[2].match(/;width=(\d+);/)?.[1]),
		};
	}
	return undefined;
}

export class RetainedViewport {
	private blocks: RenderedBlock[] = [];
	private anchor: Anchor | undefined;
	private generation = 0;
	private offset = 0;
	private height = 0;
	private totalRows = 0;
	private followingTail = true;

	constructor(private readonly options: ViewportOptions) {}

	get state(): ViewportState {
		return { offset: this.offset, height: this.height, totalRows: this.totalRows, followingTail: this.followingTail };
	}

	invalidate(components = false): void {
		if (components) for (const block of this.options.getBlocks()) block.component.invalidate();
		this.generation++;
	}

	reset(): void {
		this.blocks = [];
		this.anchor = undefined;
		this.offset = 0;
		this.totalRows = 0;
		this.followingTail = true;
	}

	scrollTo(offset: number, screenRow = 0): void {
		if (!Number.isFinite(offset) || !Number.isFinite(screenRow)) throw new Error("Viewport positions must be finite");
		this.offset = Math.max(0, Math.min(Math.trunc(offset), this.maxOffset));
		this.followingTail = this.offset === this.maxOffset;
		const row = Math.max(0, Math.min(Math.trunc(screenRow), this.height - 1));
		this.anchor = this.followingTail ? undefined : this.anchorAt(this.offset + row, row);
	}

	private get maxOffset(): number {
		return Math.max(0, this.totalRows - this.height);
	}

	private anchorAt(row: number, screenRow: number): Anchor | undefined {
		const block = this.blocks.find(
			(candidate) => row >= candidate.start && row < candidate.start + candidate.lines.length,
		);
		if (!block) return undefined;
		const localRow = row - block.start;
		const first = comparison([block.lines[localRow]])[0];
		return { component: block.component, row: localRow, grapheme: first?.grapheme ?? 0, screenRow };
	}

	update(width: number, height: number): string[] {
		const previous = new Map(this.blocks.map((block) => [block.component, block]));
		const next = new Map<Component, RenderedBlock>();
		let start = 0;
		for (const block of this.options.getBlocks()) {
			if (next.has(block.component)) throw new Error("Viewport blocks must have unique component identities");
			const old = previous.get(block.component);
			const reusable =
				block.finalized &&
				old?.finalized &&
				old.width === width &&
				old.generation === this.generation &&
				old.revision === block.revision;
			let lines = reusable ? old.lines : [...block.component.render(width)];
			if (old && lines.length === old.lines.length && lines.every((line, i) => line === old.lines[i]))
				lines = old.lines;
			next.set(block.component, { ...block, lines, start, width, generation: this.generation });
			start += lines.length;
		}
		this.totalRows = start;
		this.height = Math.max(0, height);
		if (!this.followingTail && this.anchor) {
			const anchor = this.anchor;
			const old = previous.get(anchor.component);
			const current = next.get(anchor.component);
			let mapped = old && current ? mapAnchor(anchor, old, current) : undefined;
			if (!mapped) {
				const oldIndex = this.blocks.findIndex((block) => block.component === anchor.component);
				for (let distance = 1; distance < this.blocks.length && !mapped; distance++) {
					for (const index of [oldIndex + distance, oldIndex - distance]) {
						const adjacent = this.blocks[index];
						const surviving = adjacent && next.get(adjacent.component);
						if (surviving?.lines.length) {
							mapped = {
								component: surviving.component,
								row: index > oldIndex ? 0 : surviving.lines.length - 1,
								grapheme: 0,
								screenRow: anchor.screenRow,
							};
							break;
						}
					}
				}
			}
			this.anchor = mapped;
			if (mapped) {
				const screenRow = Math.max(0, Math.min(mapped.screenRow, this.height - 1));
				this.offset = next.get(mapped.component)!.start + mapped.row - screenRow;
			}
		}
		this.blocks = [...next.values()];
		this.offset = this.followingTail ? this.maxOffset : Math.max(0, Math.min(this.offset, this.maxOffset));
		if (!this.followingTail && !this.anchor) this.anchor = this.anchorAt(this.offset, 0);
		const lines: string[] = [];
		for (const block of this.blocks) for (const line of block.lines) lines.push(line);
		return lines;
	}

	slice(logical: string[], width: number, hideImages: boolean): { lines: string[]; images: ImagePlacement[] } {
		const lines = logical.slice(this.offset, this.offset + this.height);
		const images: ImagePlacement[] = [];
		for (let row = 0; row < logical.length; row++) {
			if (!isImageLine(logical[row])) continue;
			const placement = imagePlacement(logical[row], row);
			const valid =
				placement &&
				Number.isSafeInteger(placement.rows) &&
				placement.rows > 0 &&
				Number.isSafeInteger(placement.columns) &&
				placement.columns > 0 &&
				placement.row >= 0;
			const top = valid ? placement.row : row;
			const bottom = valid ? top + placement.rows : row + 1;
			if (top >= this.offset + this.height || bottom <= this.offset) continue;
			if (row >= this.offset && row < this.offset + this.height) lines[row - this.offset] = "";
			if (
				valid &&
				top >= this.offset &&
				bottom <= this.offset + this.height &&
				placement.col + placement.columns <= width &&
				!hideImages
			) {
				images.push({ ...placement, row: top - this.offset });
			} else {
				const label = hideImages ? "[Image hidden by overlay]" : "[Image clipped; scroll to view]";
				lines[Math.max(0, top - this.offset)] = truncateToWidth(label, width);
			}
		}
		return { lines, images };
	}

	scrollbar(row: number): string {
		if (this.totalRows <= this.height) return " ";
		const size = Math.max(1, Math.floor((this.height * this.height) / this.totalRows));
		const top = Math.round((this.offset / this.maxOffset) * (this.height - size));
		return row >= top && row < top + size ? "█" : "│";
	}
}
