import { getKeybindings, truncateToWidth, visibleWidth, wrapTextWithAnsi } from "@leanandmean/tui";

export interface MultiLineSelectItem {
	value: string;
	label: string;
	description?: string;
}

export interface MultiLineSelectTheme {
	selectedText: (text: string) => string;
	description: (text: string) => string;
	scrollInfo: (text: string) => string;
}

export interface MultiLineSelectOptions {
	recommendedIndex?: number;
	maxLinesPerField?: number;
}

const PREFIX_SELECTED = "→ ";
const PREFIX_NORMAL = "  ";
const DESCRIPTION_INDENT = "     ";
const MAX_LINES_PER_FIELD = 4;

export class MultiLineSelectList {
	private items: MultiLineSelectItem[];
	private selectedIndex = 0;
	private maxVisible: number;
	private maximumRows: number | undefined;
	private rowAnchor = { itemIndex: 0, rowOffset: 0 };
	private theme: MultiLineSelectTheme;
	private recommendedIndex: number;
	private maxLinesPerField: number;

	onSelect?: (item: MultiLineSelectItem) => void;
	onCancel?: () => void;
	onSelectionChange?: (item: MultiLineSelectItem) => void;

	constructor(
		items: MultiLineSelectItem[],
		maxVisible: number,
		theme: MultiLineSelectTheme,
		options: MultiLineSelectOptions = {},
	) {
		this.items = items;
		this.maxVisible = maxVisible;
		this.theme = theme;
		this.recommendedIndex = options.recommendedIndex ?? -1;
		this.maxLinesPerField = options.maxLinesPerField ?? MAX_LINES_PER_FIELD;
	}

	setSelectedIndex(index: number): void {
		this.selectedIndex = Math.max(0, Math.min(index, this.items.length - 1));
	}

	setMaxHeight(rows: number | undefined): void {
		this.maximumRows = rows === undefined ? undefined : Math.max(0, Math.floor(rows));
	}

	invalidate(): void {}

	render(width: number): string[] {
		if (this.items.length === 0) return [];
		if (this.maximumRows !== undefined) return this.renderRowWindow(width, this.maximumRows);

		const { startIndex, endIndex } = this.computeVisibleRange();
		const lines: string[] = [];

		for (let i = startIndex; i < endIndex; i++) {
			const item = this.items[i];
			if (!item) continue;
			lines.push(...this.renderItem(item, i, width));
		}

		if (startIndex > 0 || endIndex < this.items.length) {
			lines.push(this.theme.scrollInfo(`  (${this.selectedIndex + 1}/${this.items.length})`));
		}

		return lines;
	}

	handleInput(data: string): void {
		const kb = getKeybindings();

		if (kb.matches(data, "tui.select.up")) {
			this.selectedIndex = this.selectedIndex === 0 ? this.items.length - 1 : this.selectedIndex - 1;
			this.notifySelectionChange();
		} else if (kb.matches(data, "tui.select.down")) {
			this.selectedIndex = this.selectedIndex === this.items.length - 1 ? 0 : this.selectedIndex + 1;
			this.notifySelectionChange();
		} else if (kb.matches(data, "tui.select.confirm")) {
			const item = this.items[this.selectedIndex];
			if (item && this.onSelect) this.onSelect(item);
		} else if (kb.matches(data, "tui.select.cancel")) {
			if (this.onCancel) this.onCancel();
		}
	}

	getSelectedItem(): MultiLineSelectItem | null {
		return this.items[this.selectedIndex] ?? null;
	}

	private renderRowWindow(width: number, maximumRows: number): string[] {
		const rendered = new Map<number, string[]>();
		const itemRows = (index: number): string[] => {
			let rows = rendered.get(index);
			if (!rows) {
				rows = this.renderItem(this.items[index], index, width);
				rendered.set(index, rows);
			}
			return rows;
		};
		const selected = itemRows(this.selectedIndex);
		if (selected.length >= maximumRows) {
			this.rowAnchor = { itemIndex: this.selectedIndex, rowOffset: 0 };
			return selected;
		}

		const maxItems = Math.max(1, this.maxVisible);
		const origin = {
			itemIndex: this.rowAnchor.itemIndex,
			rowOffset: Math.min(this.rowAnchor.rowOffset, itemRows(this.rowAnchor.itemIndex).length - 1),
		};
		const read = (start: typeof origin, capacity: number) => {
			const lines: string[] = [];
			let { itemIndex, rowOffset } = start;
			const lastItem = Math.min(this.items.length, itemIndex + maxItems);
			while (itemIndex < lastItem && lines.length < capacity) {
				const rows = itemRows(itemIndex);
				const take = Math.min(rows.length - rowOffset, capacity - lines.length);
				lines.push(...rows.slice(rowOffset, rowOffset + take));
				rowOffset += take;
				if (rowOffset === rows.length) {
					itemIndex++;
					rowOffset = 0;
				}
			}
			return { lines, end: { itemIndex, rowOffset } };
		};
		const layout = (capacity: number) => {
			let start = { ...origin };
			if (start.itemIndex > this.selectedIndex || (start.itemIndex === this.selectedIndex && start.rowOffset > 0))
				start = { itemIndex: this.selectedIndex, rowOffset: 0 };
			let window = read(start, capacity);
			if (window.end.itemIndex <= this.selectedIndex) {
				let remaining = capacity;
				start = { itemIndex: this.selectedIndex, rowOffset: selected.length };
				const firstItem = Math.max(0, this.selectedIndex - maxItems + 1);
				while (remaining > 0) {
					const take = Math.min(start.rowOffset, remaining);
					start.rowOffset -= take;
					remaining -= take;
					if (remaining === 0 || start.itemIndex === firstItem) break;
					start.itemIndex--;
					start.rowOffset = itemRows(start.itemIndex).length;
				}
				window = read(start, capacity);
			}
			return { ...window, start };
		};

		let window = layout(maximumRows);
		const hidden =
			window.start.itemIndex > 0 || window.start.rowOffset > 0 || window.end.itemIndex < this.items.length;
		if (hidden) {
			window = layout(maximumRows - 1);
			window.lines.push(
				this.theme.scrollInfo(truncateToWidth(`  (${this.selectedIndex + 1}/${this.items.length})`, width, "")),
			);
		}
		this.rowAnchor = window.start;
		return window.lines;
	}

	private computeVisibleRange(): { startIndex: number; endIndex: number } {
		const total = this.items.length;
		if (total <= this.maxVisible) return { startIndex: 0, endIndex: total };

		const half = Math.floor(this.maxVisible / 2);
		const startIndex = Math.max(0, Math.min(this.selectedIndex - half, total - this.maxVisible));
		const endIndex = startIndex + this.maxVisible;
		return { startIndex, endIndex };
	}

	private renderItem(item: MultiLineSelectItem, index: number, width: number): string[] {
		const isSelected = index === this.selectedIndex;
		const prefix = isSelected ? PREFIX_SELECTED : PREFIX_NORMAL;
		const prefixWidth = visibleWidth(prefix);
		const labelWidth = Math.max(1, width - prefixWidth);
		const lines: string[] = [];

		const labelLines = this.wrapAndCap(item.label, labelWidth);
		for (let i = 0; i < labelLines.length; i++) {
			const linePrefix = i === 0 ? prefix : " ".repeat(prefixWidth);
			const line = linePrefix + labelLines[i];
			lines.push(isSelected ? this.theme.selectedText(line) : line);
		}

		if (item.description) {
			const indentWidth = visibleWidth(DESCRIPTION_INDENT);
			const descWidth = Math.max(1, width - indentWidth);
			const descText = index === this.recommendedIndex ? `[recommended] ${item.description}` : item.description;
			const descLines = this.wrapAndCap(descText, descWidth);
			for (const dLine of descLines) {
				const line = DESCRIPTION_INDENT + dLine;
				lines.push(isSelected ? this.theme.selectedText(line) : this.theme.description(line));
			}
		}

		return lines;
	}

	private wrapAndCap(text: string, width: number): string[] {
		const wrapped = wrapTextWithAnsi(text, width);
		if (wrapped.length <= this.maxLinesPerField) return wrapped;
		const capped = wrapped.slice(0, this.maxLinesPerField);
		capped[this.maxLinesPerField - 1] = `${truncateToWidth(capped[this.maxLinesPerField - 1]!, width - 1, "")}…`;
		return capped;
	}

	private notifySelectionChange(): void {
		const item = this.items[this.selectedIndex];
		if (item && this.onSelectionChange) this.onSelectionChange(item);
	}
}
