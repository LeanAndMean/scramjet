import { DynamicBorder, type Theme } from "@leanandmean/coding-agent";
import { getRenderedCopy, setRenderedCopy, Text } from "@leanandmean/tui";
import type { MultiLineSelectList } from "./multi-line-select.js";

export function renderSelectorFrame({
	width,
	maximumRows,
	title,
	list,
	tailRows,
	theme,
}: {
	width: number;
	maximumRows: number | undefined;
	title: string;
	list: MultiLineSelectList;
	tailRows: readonly string[];
	theme: Pick<Theme, "fg" | "bold">;
}): string[] {
	const border = new DynamicBorder((text) => theme.fg("border", text)).render(width);
	const heading = new Text(theme.fg("accent", theme.bold(title)), 0, 0).render(width);
	const tail = tailRows.map((row) => new Text(row, 0, 0).render(width));
	const chromeHeight = border.length * 2 + heading.length + tail.reduce((height, rows) => height + rows.length, 0);
	list.setMaxHeight(maximumRows === undefined ? undefined : Math.max(0, maximumRows - chromeHeight));
	const parts = [heading, list.render(width), ...tail];
	return setRenderedCopy(
		[...border, ...parts.flat(), ...border],
		[null, ...parts.flatMap((rows) => getRenderedCopy(rows)), null],
	);
}
