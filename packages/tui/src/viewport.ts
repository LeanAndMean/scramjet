// SCRAMJET-DIVERGENCE: retained component rows and content-relative reading anchors.
import { diffArrays } from "diff";
import { type ImagePlacement, sliceImagePlacements } from "./image-placement.js";
import { getKeybindings, type KeybindingsManager } from "./keybindings.js";
import { isKeyModifier, isKeyRelease, matchesKey } from "./keys.js";
import { getRenderedCopy, hasRenderedCopy, type RenderedCopyRow } from "./render-copy.js";
import { isImageLine } from "./terminal-image.js";
import { type Component, CURSOR_MARKER } from "./tui.js";
import {
	extractAnsiCode,
	getSegmenter,
	normalizeTerminalOutput,
	sliceByColumn,
	truncateToWidth,
	visibleWidth,
} from "./utils.js";

export interface ViewportBlock {
	component: Component;
	finalized?: boolean;
	dock?: boolean;
	fitHeight?(rows: number): void;
	/** Increment when a finalized component's presentation changes. */
	revision?: number;
}

export interface ViewportOptions {
	getBlocks(): readonly ViewportBlock[];
	keybindings?: KeybindingsManager;
	copy?(text: string): Promise<void>;
	getScrollWheelStep?(): number;
	handlePresentationInput?(data: string): boolean;
	keepReadingOnInput?(): boolean;
	allowViewportKeys?(data: string): boolean;
	minimumSize?: { columns: number; rows: number };
	handleBlockedInput?(data: string): void;
}

export interface ViewportState {
	offset: number;
	totalRows: number;
	height: number;
	followingTail: boolean;
}

interface RenderedBlock extends ViewportBlock {
	lines: string[];
	copyRows: readonly RenderedCopyRow[];
	rawRows: string[];
	sourceCopy: readonly RenderedCopyRow[] | undefined;
	complete: boolean;
	hasImages: boolean;
	cursorRow: number;
	start: number;
	width: number;
	height: number;
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
	maxEditLength?: number,
): { position: number; exact: boolean } | undefined {
	let oldIndex = 0;
	let newIndex = 0;
	let nearest: { position: number; exact: boolean } | undefined;
	let distance = Infinity;
	const changes = maxEditLength === undefined ? diffArrays(old, next) : diffArrays(old, next, { maxEditLength });
	if (!changes) return undefined;
	for (const change of changes) {
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
	// An unchanged prefix survives even when later edits exhaust the correspondence budget.
	for (let row = 0; row <= anchor.row && row < next.lines.length; row++) {
		if (old.lines[row] !== next.lines[row]) break;
		if (row === anchor.row) return anchor;
	}
	const rows = correspondence(old.lines, next.lines, anchor.row, 64);
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
			64,
		);
		if (mapped && (mapped.exact || !rows)) {
			const token = newTokens[mapped.position];
			return { ...anchor, row: token.row, grapheme: token.grapheme };
		}
	}
	return rows ? { ...anchor, row: rows.position, grapheme: 0 } : undefined;
}

interface SelectionPoint {
	row: number;
	column: number;
}

function plainText(line: string): string {
	let result = "";
	for (let i = 0; i < line.length; ) {
		const ansi = extractAnsiCode(line, i);
		if (ansi) i += ansi.length;
		else {
			const char = line[i++];
			if (char >= " " && !(char >= "\x7f" && char <= "\x9f")) result += char;
		}
	}
	return result;
}

export class RetainedViewport {
	private blocks: RenderedBlock[] = [];
	private anchor: Anchor | undefined;
	private generation = 0;
	private offset = 0;
	private height = 0;
	private paintedOffset = 0;
	private paintedHeight = 0;
	private paintedCopyErrorRow: number | undefined;
	private totalRows = 0;
	private followingTail = true;
	private width = 0;
	private terminalColumns = 0;
	private screenHeight = 0;
	private logicalRows: string[] | undefined;
	private logicalCopy: readonly RenderedCopyRow[] = [];
	private dockHeight = 0;
	private paintedDockTop = 0;
	private paintedDockStart = 0;
	private dockSuspended = false;
	private selectionInDock = false;
	private visibleComponents = new Set<Component>();
	private selection: { start: SelectionPoint; end: SelectionPoint; followOnCopy: boolean } | undefined;
	private copyError: string | undefined;
	private copying = false;
	private gesture:
		| { kind: "thumb"; grab: number; travel: number; maximum: number }
		| { kind: "selection"; dragged: boolean; scrolled: boolean }
		| undefined;
	private edgeTimer: ReturnType<typeof setInterval> | undefined;
	private edgeDirection = 0;
	private pointerColumn = 0;

	constructor(
		private readonly options: ViewportOptions,
		private readonly requestRender: () => void = () => {},
	) {}

	isTooSmall(columns = this.terminalColumns, rows = this.screenHeight): boolean {
		return (
			!!this.options.minimumSize &&
			(columns < this.options.minimumSize.columns || rows < this.options.minimumSize.rows)
		);
	}

	get notice(): string | undefined {
		if (this.isTooSmall()) return "Resize";
		if (this.copyError) return `Copy failed: ${this.copyError}; selection retained`;
		return this.dockSuspended ? "Dock suspended: insufficient space" : undefined;
	}

	cancelInteraction(): void {
		this.endGesture();
		this.selection = undefined;
		this.copyError = undefined;
		this.copying = false;
	}

	private endGesture(): void {
		this.gesture = undefined;
		this.edgeDirection = 0;
		if (this.edgeTimer) clearInterval(this.edgeTimer);
		this.edgeTimer = undefined;
	}

	private point(x: number, y: number, offset = this.paintedOffset, height = this.paintedHeight): SelectionPoint {
		const row = this.selectionInDock
			? Math.max(
					this.paintedDockStart,
					Math.min(this.logical.length - 1, this.paintedDockStart + y - this.paintedDockTop),
				)
			: Math.max(0, Math.min(this.totalRows - 1, offset + Math.min(y, height - 1)));
		const text = plainText(this.logical[row] ?? "");
		let column = 0;
		for (const { segment } of getSegmenter().segment(text)) {
			const size = visibleWidth(segment);
			if (column + size > x) break;
			column += size;
		}
		return { row, column };
	}

	private selectionRange(): [SelectionPoint, SelectionPoint] | undefined {
		if (!this.selection) return undefined;
		const { start, end } = this.selection;
		return start.row < end.row || (start.row === end.row && start.column <= end.column) ? [start, end] : [end, start];
	}

	private selectedText(): string {
		const range = this.selectionRange();
		if (!range) return "";
		const [start, end] = range;
		let result = "";
		let previous: Exclude<RenderedCopyRow, null> | undefined;
		for (let index = start.row; index <= end.row; index++) {
			const line = this.logical[index];
			const source = this.logicalCopy[index];
			if (!source || isImageLine(line)) continue;
			const left = Math.max(source.start, index === start.row ? start.column : 0);
			const right = Math.min(source.end, index === end.row ? end.column : source.end);
			if (right < left || (right === left && source.start !== source.end)) continue;
			if (previous) result += previous.after ?? "\n";
			result += sliceByColumn(plainText(line), left, right - left, true);
			previous = source;
		}
		return result;
	}

	private async copySelection(): Promise<void> {
		const text = this.selectedText();
		if (!text || this.copying) return;
		const selection = this.selection;
		this.copying = true;
		this.endGesture();
		try {
			if (!this.options.copy) throw new Error("No clipboard callback configured");
			await this.options.copy(text);
			if (this.selection !== selection) return;
			const resumeTail = selection?.followOnCopy && this.offset === this.maxOffset;
			this.cancelInteraction();
			if (resumeTail) this.scrollTo(this.maxOffset);
		} catch (error) {
			if (this.selection !== selection) return;
			this.copyError = plainText(error instanceof Error ? error.message : String(error));
			this.copying = false;
		}
		this.requestRender();
	}

	handleInput(data: string, overlayFocused: boolean, overlayVisible: boolean): boolean {
		if (data === "\x1b[O") {
			if (this.gesture?.kind === "selection") this.cancelInteraction();
			else this.endGesture();
			this.requestRender();
			return true;
		}
		if (data === "\x1b[I" || isKeyModifier(data)) return true;
		if (data.startsWith("\x1b[<") || data.startsWith("\x1b[M")) {
			if (overlayVisible) {
				this.endGesture();
				return true;
			}
			const match = /^\x1b\[<(\d{1,3});(\d{1,5});(\d{1,5})([Mm])$/.exec(data);
			if (!match) return true;
			const [button, x, y] = match.slice(1, 4).map(Number);
			if (match[4] === "m") {
				const wasThumb = this.gesture?.kind === "thumb";
				this.endGesture();
				if (wasThumb) this.scrollTo(this.offset);
				if (
					this.selection &&
					this.selection.start.row === this.selection.end.row &&
					this.selection.start.column === this.selection.end.column
				)
					this.cancelInteraction();
				this.requestRender();
				return true;
			}
			if (x < 1 || x > this.width + 1 || y < 1 || y > this.screenHeight) return true;
			if (y - 1 === this.paintedCopyErrorRow && button !== 2) {
				if (button === 0) this.copyError = undefined;
				this.endGesture();
				this.requestRender();
				return true;
			}
			if (button === 64 || button === 65) {
				this.scrollTo(this.offset + (button === 64 ? -1 : 1) * (this.options.getScrollWheelStep?.() ?? 3));
				if (this.gesture?.kind === "selection" && this.selection && !this.selectionInDock) {
					this.gesture.scrolled = true;
					if (this.gesture.dragged) {
						this.pointerColumn = Math.min(x - 1, this.width);
						this.edgeDirection = y >= this.height ? 1 : y === 1 ? -1 : 0;
						this.selection.end = this.point(this.pointerColumn, y - 1, this.offset, this.height);
					}
				}
			} else if (button === 2) {
				void this.copySelection();
			} else if (button === 0) {
				this.endGesture();
				if (x === this.width + 1 && y <= this.paintedHeight) {
					this.cancelInteraction();
					this.height = this.screenHeight - this.dockHeight - (this.dockSuspended ? 1 : 0);
					const { size, top } = this.thumb();
					const grab = y - 1 >= top && y - 1 < top + size ? y - 1 - top : Math.floor(size / 2);
					this.gesture = { kind: "thumb", grab, travel: this.height - size, maximum: this.maxOffset };
					this.dragThumb(y - 1);
				} else if (
					(y <= this.paintedHeight || (this.dockHeight > 0 && y > this.paintedDockTop)) &&
					this.totalRows + this.dockHeight > 0 &&
					this.screenHeight > 1
				) {
					this.cancelInteraction();
					this.selectionInDock = this.dockHeight > 0 && y > this.paintedDockTop;
					this.logicalCopy = this.blocks.flatMap((block) => block.copyRows);
					const point = this.point(x - 1, y - 1);
					this.offset = this.paintedOffset;
					this.selection = { start: point, end: point, followOnCopy: this.followingTail };
					this.followingTail = false;
					this.anchor = this.anchorAt(this.offset, 0);
					this.gesture = { kind: "selection", dragged: false, scrolled: false };
				}
			} else if (button === 32) {
				if (this.gesture?.kind === "thumb") this.dragThumb(y - 1);
				else if (this.gesture?.kind === "selection" && this.selection) {
					this.gesture.dragged = true;
					this.pointerColumn = Math.min(x - 1, this.width);
					this.selection.end = this.point(
						this.pointerColumn,
						y - 1,
						this.gesture.scrolled ? this.offset : this.paintedOffset,
						this.gesture.scrolled ? this.height : this.paintedHeight,
					);
					this.edgeDirection = this.selectionInDock ? 0 : y >= this.height ? 1 : y === 1 ? -1 : 0;
					if (!this.edgeTimer)
						this.edgeTimer = setInterval(() => {
							if (!this.selection || !this.edgeDirection || this.gesture?.kind !== "selection") return;
							const previousOffset = this.offset;
							this.scrollTo(this.offset + this.edgeDirection);
							if (this.offset === previousOffset) {
								clearInterval(this.edgeTimer);
								this.edgeTimer = undefined;
								return;
							}
							this.gesture.scrolled = true;
							this.selection.end = this.point(
								this.pointerColumn,
								this.edgeDirection > 0 ? this.height - 1 : 0,
								this.offset,
								this.height,
							);
							this.requestRender();
						}, 80);
				}
			}
			this.requestRender();
			return true;
		}
		if (overlayFocused || isKeyRelease(data) || /^\x1b\[\d+;\d+;\d+t$/.test(data)) return false;
		if (this.selection && (this.options.keybindings ?? getKeybindings()).matches(data, "tui.input.copy")) {
			void this.copySelection();
			this.requestRender();
			return true;
		}
		const keybindings = this.options.keybindings ?? getKeybindings();
		const allowNavigation = this.options.allowViewportKeys?.(data) !== false;
		if (
			allowNavigation &&
			(keybindings.matches(data, "tui.viewport.pageUp") || keybindings.matches(data, "tui.viewport.pageDown"))
		) {
			const direction = keybindings.matches(data, "tui.viewport.pageUp") ? -1 : 1;
			this.scrollTo(this.offset + direction * this.height);
			this.requestRender();
			return true;
		}
		if (allowNavigation && (matchesKey(data, "ctrl+home") || matchesKey(data, "ctrl+end"))) {
			this.cancelInteraction();
			this.scrollTo(matchesKey(data, "ctrl+end") ? this.maxOffset : 0);
			this.requestRender();
			return true;
		}
		const detached = !this.followingTail;
		if (matchesKey(data, "escape") && this.selection) {
			this.cancelInteraction();
			this.scrollTo(this.maxOffset);
			this.requestRender();
			return true;
		}
		if (!allowNavigation) {
			if (this.selection) this.cancelInteraction();
			return false;
		}
		if (detached) {
			if (this.options.handlePresentationInput?.(data)) {
				this.cancelInteraction();
				this.requestRender();
				return true;
			}
			if (matchesKey(data, "pageUp")) this.scrollTo(this.offset - this.height);
			else if (matchesKey(data, "pageDown")) this.scrollTo(this.offset + this.height);
			else {
				this.cancelInteraction();
				const returnToTail = matchesKey(data, "escape");
				if (returnToTail || !this.dockHeight || !this.options.keepReadingOnInput?.()) this.scrollTo(this.maxOffset);
				this.requestRender();
				return returnToTail;
			}
			this.requestRender();
			return true;
		}
		if (this.selection) {
			this.cancelInteraction();
			this.requestRender();
		}
		return false;
	}

	private dragThumb(row: number): void {
		if (this.gesture?.kind !== "thumb") return;
		const { grab, travel, maximum } = this.gesture;
		this.scrollTo(Math.round((Math.max(0, Math.min(travel, row - grab)) / Math.max(1, travel)) * maximum));
	}

	private thumb(): { size: number; top: number } {
		if (this.gesture?.kind === "thumb") {
			const { travel, maximum } = this.gesture;
			return { size: this.height - travel, top: Math.round((this.offset / Math.max(1, maximum)) * travel) };
		}
		const size = Math.min(
			this.height,
			Math.max(1, Math.floor((this.height * this.height) / Math.max(1, this.totalRows))),
		);
		const top = Math.round((this.offset / Math.max(1, this.maxOffset)) * (this.height - size));
		return { size, top };
	}

	get state(): ViewportState {
		return { offset: this.offset, height: this.height, totalRows: this.totalRows, followingTail: this.followingTail };
	}

	invalidate(components = false): void {
		if (components) for (const block of this.options.getBlocks()) block.component.invalidate();
		this.generation++;
	}

	reset(): void {
		this.cancelInteraction();
		this.logicalRows = undefined;
		this.logicalCopy = [];
		this.blocks = [];
		this.visibleComponents.clear();
		this.dockHeight = 0;
		this.dockSuspended = false;
		this.anchor = undefined;
		this.offset = 0;
		this.paintedOffset = 0;
		this.paintedHeight = 0;
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

	isComponentVisible(component: Component): boolean {
		return this.visibleComponents.has(component);
	}

	isComponentRenderComplete(component: Component): boolean {
		const block = this.blocks.find((block) => block.component === component);
		return !this.isTooSmall() && !this.selection && block?.generation === this.generation && block.complete;
	}

	revealComponent(component: Component): void {
		const block = this.blocks.find((block) => block.component === component);
		if (block && block.start < this.totalRows)
			this.scrollTo(block.start + Math.max(0, block.lines.length - this.height));
	}

	revealRow(row: number): void {
		if (row >= this.totalRows && this.dockHeight) return;
		if (row < this.offset) this.scrollTo(row);
		else if (row >= this.offset + this.height) this.scrollTo(row - this.height + 1);
	}

	get noticeRow(): number {
		return this.isTooSmall() ? 0 : this.copyError ? this.screenHeight - 1 : this.height;
	}

	update(width: number, height: number, terminalColumns = width + 1): void {
		if (width !== this.width || height !== this.screenHeight) this.cancelInteraction();
		this.width = width;
		this.terminalColumns = terminalColumns;
		this.screenHeight = height;
		if (this.isTooSmall()) {
			this.height = 0;
			this.visibleComponents.clear();
			return;
		}
		const previous = new Map(this.blocks.map((block) => [block.component, block]));
		const next = new Map<Component, RenderedBlock>();
		const projected = this.options.getBlocks();
		if (new Set(projected.map((block) => block.component)).size !== projected.length)
			throw new Error("Viewport blocks must have unique component identities");
		const render = (block: ViewportBlock, availableHeight: number): RenderedBlock => {
			block.fitHeight?.(availableHeight);
			const old = previous.get(block.component);
			const reusable =
				block.finalized &&
				old?.finalized &&
				old.width === width &&
				old.height === availableHeight &&
				old.generation === this.generation &&
				old.revision === block.revision;
			block.component.setViewportHeight?.(availableHeight);
			const rendered = reusable ? old.rawRows : block.component.render(width);
			const sourceCopy = reusable
				? old.sourceCopy
				: hasRenderedCopy(rendered)
					? getRenderedCopy(rendered)
					: undefined;
			if (
				reusable ||
				(old &&
					old.width === width &&
					old.height === availableHeight &&
					sourceCopy === old.sourceCopy &&
					old.rawRows.length === rendered.length &&
					old.rawRows.every((line, index) => line === rendered[index]))
			)
				return {
					...block,
					lines: old.lines,
					copyRows: old.copyRows,
					rawRows: old.rawRows,
					sourceCopy,
					complete: old.complete,
					hasImages: old.hasImages,
					cursorRow: old.cursorRow,
					start: 0,
					width,
					height: availableHeight,
					generation: this.generation,
				};
			let complete = true;
			let lines = rendered.map((line) => {
				if (isImageLine(line)) return line;
				const normalized = normalizeTerminalOutput(line).replaceAll("\t", "   ");
				if (visibleWidth(normalized) <= width) return normalized;
				complete = false;
				return sliceByColumn(normalized, 0, width, true);
			});
			if (!complete) lines.push(truncateToWidth("Clipped output: component exceeded width", width, ""));
			let hasImages = false;
			let cursorRow = -1;
			const copyRows = lines.map((line, index) => {
				if (isImageLine(line)) hasImages = true;
				if (cursorRow === -1 && line.includes(CURSOR_MARKER)) cursorRow = index;
				const row = sourceCopy?.[index];
				const size = visibleWidth(line);
				if (row === undefined || rendered[index]?.includes("\t")) return { start: 0, end: size };
				return row === null
					? null
					: {
							start: Math.min(row.start, size),
							end: Math.min(row.end, size),
							after: visibleWidth(rendered[index]) > width ? undefined : row.after,
						};
			});
			if (old && lines.length === old.lines.length && lines.every((line, i) => line === old.lines[i]))
				lines = old.lines;
			return {
				...block,
				lines,
				copyRows,
				rawRows: [...rendered],
				sourceCopy,
				complete,
				hasImages,
				cursorRow,
				start: 0,
				width,
				height: availableHeight,
				generation: this.generation,
			};
		};
		const firstDock = projected.findIndex((block) => block.dock);
		if (firstDock !== -1 && projected.slice(firstDock).some((block) => !block.dock))
			throw new Error("Dock blocks must form the document suffix");
		const dock = new Map<Component, RenderedBlock>();
		for (const block of projected.filter((block) => block.dock && !block.fitHeight))
			dock.set(block.component, render(block, Math.max(1, height - 1)));
		const fixedDockHeight = [...dock.values()].reduce((sum, block) => sum + block.lines.length, 0);
		for (const block of projected.filter((block) => block.dock && block.fitHeight))
			dock.set(
				block.component,
				render(block, Math.max(1, fixedDockHeight >= height - 2 ? height - 1 : height - fixedDockHeight - 2)),
			);
		const proposedDockHeight = [...dock.values()].reduce((sum, block) => sum + block.lines.length, 0);
		const suspended = proposedDockHeight > 0 && proposedDockHeight + 2 > height;
		const dockHeight = suspended ? 0 : proposedDockHeight;
		const renderHeight = Math.max(1, height - dockHeight - (suspended ? 1 : 0));
		let start = 0;
		for (const block of projected) {
			const rendered = block.dock
				? suspended && block.fitHeight && dock.get(block.component)!.height !== renderHeight
					? render(block, renderHeight)
					: dock.get(block.component)!
				: render(block, renderHeight);
			rendered.start = start;
			next.set(block.component, rendered);
			start += rendered.lines.length;
		}
		if (this.selection) return;
		this.dockHeight = dockHeight;
		this.dockSuspended = suspended;
		this.height = renderHeight;
		this.totalRows = start - dockHeight;
		if (this.gesture?.kind !== "thumb" && !this.followingTail && this.anchor) {
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
		this.offset =
			this.followingTail && this.gesture?.kind !== "thumb"
				? this.maxOffset
				: Math.max(0, Math.min(this.offset, this.maxOffset));
		if (!this.followingTail && !this.anchor) this.anchor = this.anchorAt(this.offset, 0);
		this.logicalRows = undefined;
		this.logicalCopy = [];
	}

	get cursorRow(): number {
		const block = this.blocks.find((block) => block.cursorRow >= 0);
		return block ? block.start + block.cursorRow : -1;
	}

	private get logical(): string[] {
		if (!this.logicalRows) {
			this.logicalRows = [];
			for (const block of this.blocks) for (const line of block.lines) this.logicalRows.push(line);
		}
		return this.logicalRows;
	}

	private sliceRows(start: number, end: number): string[] {
		const lines: string[] = [];
		for (const block of this.blocks) {
			if (block.start >= end) break;
			const from = Math.max(0, start - block.start);
			const to = Math.min(block.lines.length, end - block.start);
			for (let row = from; row < to; row++) lines.push(block.lines[row]);
		}
		return lines;
	}

	slice(width: number, hideImages: boolean): { lines: string[]; images: ImagePlacement[] } {
		if (this.isTooSmall()) {
			this.visibleComponents.clear();
			return { lines: [], images: [] };
		}
		this.visibleComponents = new Set(
			this.blocks
				.filter(
					(block) =>
						!this.selection &&
						block.lines.length > 0 &&
						((this.dockHeight > 0 && block.start >= this.totalRows) ||
							(block.start >= this.offset && block.start + block.lines.length <= this.offset + this.height)),
				)
				.map((block) => block.component),
		);
		const hiddenLabel = hideImages
			? "[Image hidden by overlay]"
			: this.selection
				? "[Image hidden by selection]"
				: undefined;
		// iTerm placements can start above their payload row, including outside the visible slice.
		const logical = this.selection || this.blocks.some((block) => block.hasImages) ? this.logical : undefined;
		const { lines, images } = logical
			? sliceImagePlacements(logical.slice(0, this.totalRows), this.offset, this.height, width, hiddenLabel)
			: {
					lines: this.sliceRows(this.offset, Math.min(this.totalRows, this.offset + this.height)),
					images: [] as ImagePlacement[],
				};
		const dockTop = this.screenHeight - this.dockHeight;
		if (this.dockHeight > 0) {
			while (lines.length < dockTop) lines.push("");
			const dock = logical
				? sliceImagePlacements(logical.slice(this.totalRows), 0, this.dockHeight, width, hiddenLabel)
				: { lines: this.sliceRows(this.totalRows, this.totalRows + this.dockHeight), images: [] };
			lines.push(...dock.lines);
			images.push(...dock.images.map((image) => ({ ...image, row: image.row + dockTop })));
		}
		const range = this.selectionRange();
		if (range) {
			const [start, end] = range;
			for (let i = 0; i < lines.length; i++) {
				const inDock = this.dockHeight > 0 && i >= dockTop;
				if (inDock !== this.selectionInDock || (!inDock && i >= this.height)) continue;
				const row = inDock ? this.totalRows + i - dockTop : this.offset + i;
				if (row < start.row || row > end.row) continue;
				const left = row === start.row ? start.column : 0;
				const right = row === end.row ? end.column : visibleWidth(lines[i]);
				const selected = sliceByColumn(plainText(lines[i]), left, Math.max(0, right - left), true);
				lines[i] =
					`${sliceByColumn(lines[i], 0, left, true)}\x1b[0m\x1b[7m${selected}\x1b[0m${sliceByColumn(lines[i], right, Math.max(0, width - right), true)}`;
			}
		}
		return { lines, images };
	}

	markPainted(): void {
		this.paintedOffset = this.offset;
		this.paintedHeight = this.height;
		this.paintedCopyErrorRow = this.copyError ? this.noticeRow : undefined;
		this.paintedDockTop = this.screenHeight - this.dockHeight;
		this.paintedDockStart = this.totalRows;
	}

	scrollbar(row: number): string {
		if (this.isTooSmall() || this.totalRows <= this.height || row >= this.height) return " ";
		const { size, top } = this.thumb();
		return row >= top && row < top + size ? "█" : "│";
	}
}
