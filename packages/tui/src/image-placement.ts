// SCRAMJET-DIVERGENCE: transcript and overlay slices share atomic built-in graphics geometry.
import { isImageLine } from "./terminal-image.js";
import { truncateToWidth, visibleWidth } from "./utils.js";

export interface ImagePlacement {
	sequence: string;
	row: number;
	col: number;
	rows: number;
	columns: number;
}

function imagePlacement(line: string, row: number): ImagePlacement | undefined {
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

export function sliceImagePlacements(
	logical: string[],
	offset: number,
	height: number,
	width: number,
	hiddenLabel?: string,
): { lines: string[]; images: ImagePlacement[] } {
	const lines = logical.slice(offset, offset + height);
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
			placement.row >= 0 &&
			placement.row + placement.rows <= logical.length;
		const top = valid ? placement.row : row;
		const bottom = valid ? top + placement.rows : row + 1;
		if (top >= offset + height || bottom <= offset) continue;
		if (row >= offset && row < offset + height) lines[row - offset] = "";
		if (
			valid &&
			top >= offset &&
			bottom <= offset + height &&
			placement.col + placement.columns <= width &&
			!hiddenLabel
		) {
			images.push({ ...placement, row: top - offset });
		} else {
			lines[Math.max(0, top - offset)] = truncateToWidth(hiddenLabel ?? "[Image clipped; scroll to view]", width);
		}
	}
	return { lines, images };
}
