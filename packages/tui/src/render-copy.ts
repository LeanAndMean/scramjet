// SCRAMJET-DIVERGENCE: copy provenance belongs to the exact rendered output, not reconstructed source text.
import { isImageLine } from "./terminal-image.js";
import { sliceByColumn, visibleWidth, wrapTextWithAnsiDetailed } from "./utils.js";

export type RenderedCopyRow = { start: number; end: number; after?: string } | null;

const metadata = new WeakMap<string[], { witness: string[]; rows: readonly RenderedCopyRow[] }>();

export function setRenderedCopy(lines: string[], rows: readonly RenderedCopyRow[]): string[] {
	const previous = metadata.get(lines);
	if (
		previous &&
		hasRenderedCopy(lines) &&
		previous.rows.length === rows.length &&
		previous.rows.every((row, index) => {
			const next = rows[index];
			return (
				row === next ||
				(row != null &&
					next != null &&
					row.start === next.start &&
					row.end === next.end &&
					row.after === next.after)
			);
		})
	)
		return lines;
	const snapshot = Array.from(rows, (row) => (row === null ? null : { ...row }));
	if (
		lines.length !== snapshot.length ||
		snapshot.some(
			(row, index) =>
				row !== null &&
				(!Number.isInteger(row.start) ||
					!Number.isInteger(row.end) ||
					row.start < 0 ||
					row.end < row.start ||
					row.end > visibleWidth(lines[index]) ||
					(row.after !== undefined && (typeof row.after !== "string" || /[^\s]|[\r\n]/.test(row.after)))),
		)
	)
		throw new Error("Copy metadata must describe the rendered cells and whitespace-only wrap separators");
	metadata.set(lines, {
		witness: [...lines],
		rows: Object.freeze(snapshot.map((row) => (row === null ? null : Object.freeze(row)))),
	});
	return lines;
}

export function hasRenderedCopy(lines: string[]): boolean {
	const entry = metadata.get(lines);
	return (
		!!entry && lines.length === entry.witness.length && lines.every((line, index) => line === entry.witness[index])
	);
}

export function getRenderedCopy(lines: string[]): readonly RenderedCopyRow[] {
	if (hasRenderedCopy(lines)) return metadata.get(lines)!.rows;
	return lines.map((line) => ({ start: 0, end: visibleWidth(line) }));
}

export function wrapRenderedCopy(lines: string[], width: number): string[] {
	const result: string[] = [];
	const rows: RenderedCopyRow[] = [];
	const sources = getRenderedCopy(lines);
	for (const [index, line] of lines.entries()) {
		const source = sources[index];
		if (isImageLine(line)) {
			result.push(line);
			rows.push(null);
			continue;
		}
		const wrapped = wrapTextWithAnsiDetailed(line, width);
		let offset = 0;
		for (const [partIndex, part] of wrapped.entries()) {
			const size = visibleWidth(part.text);
			let after = partIndex === wrapped.length - 1 ? source?.after : part.after;
			if (source && after !== undefined && partIndex < wrapped.length - 1) {
				const start = Math.max(0, source.start - offset - size);
				const end = Math.min(visibleWidth(after), source.end - offset - size);
				after = sliceByColumn(after, start, Math.max(0, end - start), true);
			}
			result.push(part.text);
			rows.push(
				source === null
					? null
					: {
							start: Math.min(size, Math.max(0, source.start - offset)),
							end: Math.min(size, Math.max(0, source.end - offset)),
							after,
						},
			);
			offset += size + visibleWidth(part.after ?? "");
		}
	}
	return setRenderedCopy(result, rows);
}
