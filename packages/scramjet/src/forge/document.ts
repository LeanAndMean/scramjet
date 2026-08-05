import { createHash } from "node:crypto";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncateHead } from "@leanandmean/coding-agent";
import type {
	ForgeActor,
	ForgeArtifact,
	ForgeCoverageRange,
	ForgeFieldCoverage,
	ForgeMutationTarget,
	ForgePrSection,
	ForgeReadDetails,
	ForgeRepository,
	ForgeTextEdit,
} from "./types.js";

const FIELD_CHUNK_BYTES = 512;

export interface ForgeFieldSpan {
	line: number;
	target: ForgeMutationTarget;
	field: "title" | "body";
	start: number;
	end: number;
	totalCodeUnits: number;
}

export interface RenderedForgeDocument {
	text: string;
	lines: string[];
	snapshot: string;
	repository: ForgeRepository;
	artifact: { kind: "issue" | "pr"; number: number };
	include: ForgePrSection[];
	fieldSpans: ForgeFieldSpan[];
	coreLines: ForgeCoverageRange;
}

export interface ForgeRangeRequest {
	offset?: number;
	limit?: number;
	snapshot?: string;
}

export interface ForgeDocumentSlice {
	content: string;
	details: ForgeReadDetails;
	truncated: boolean;
	nextOffset?: number;
}

interface FieldChunk {
	text: string;
	start: number;
	end: number;
	lineBreak: "" | "&#10;" | "&#13;" | "&#13;&#10;";
}

function compareText(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

function escapeAttribute(value: string | number | boolean): string {
	return String(value)
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;")
		.replaceAll('"', "&quot;")
		.replaceAll("'", "&apos;")
		.replaceAll("\t", "&#9;")
		.replaceAll("\r", "&#13;")
		.replaceAll("\n", "&#10;");
}

function attributes(values: Array<[string, string | number | boolean | null | undefined]>): string {
	return values
		.filter((entry): entry is [string, string | number | boolean] => entry[1] !== null && entry[1] !== undefined)
		.map(([name, value]) => ` ${name}="${escapeAttribute(value)}"`)
		.join("");
}

function actorAttributes(actor: ForgeActor): Array<[string, string | null]> {
	return [
		["login", actor.login],
		["kind", actor.kind],
	];
}

function escapeCdata(value: string): string {
	let output = "";
	let plain = "";
	const flush = () => {
		output += plain.replaceAll("]]>", "]]]]><![CDATA[>").replaceAll("\t", "]]>&#9;<![CDATA[");
		plain = "";
	};

	for (let index = 0; index < value.length; index++) {
		const codeUnit = value.charCodeAt(index);
		if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
			const next = value.charCodeAt(index + 1);
			if (next >= 0xdc00 && next <= 0xdfff) {
				plain += value.slice(index, index + 2);
				index++;
				continue;
			}
		} else if (
			(codeUnit === 0x09 || codeUnit === 0x0a || codeUnit === 0x0d || codeUnit >= 0x20) &&
			codeUnit !== 0xfffe &&
			codeUnit !== 0xffff &&
			!(codeUnit >= 0xdc00 && codeUnit <= 0xdfff)
		) {
			plain += value[index];
			continue;
		}

		flush();
		output += `]]><forge-code-unit value="${codeUnit.toString(16).toUpperCase().padStart(4, "0")}"/><![CDATA[`;
	}
	flush();
	return output;
}

function fieldChunks(value: string): FieldChunk[] {
	const chunks: FieldChunk[] = [];
	let chunkStart = 0;
	let chunkText = "";
	let chunkBytes = 0;
	let index = 0;

	const push = (end: number, lineBreak: FieldChunk["lineBreak"] = "") => {
		chunks.push({ text: chunkText, start: chunkStart, end, lineBreak });
		chunkText = "";
		chunkBytes = 0;
		chunkStart = end;
	};

	while (index < value.length) {
		const char = String.fromCodePoint(value.codePointAt(index) as number);
		if (char === "\r" || char === "\n") {
			if (char === "\r" && value[index + 1] === "\n") {
				index += 2;
				push(index, "&#13;&#10;");
			} else {
				index += 1;
				push(index, char === "\r" ? "&#13;" : "&#10;");
			}
			continue;
		}

		const charBytes = Buffer.byteLength(char, "utf8");
		if (chunkText !== "" && chunkBytes + charBytes > FIELD_CHUNK_BYTES) push(index);
		chunkText += char;
		chunkBytes += charBytes;
		index += char.length;
	}
	push(value.length);
	return chunks;
}

function targetKey(target: ForgeMutationTarget): string {
	return target.kind === "artifact" ? "artifact" : `comment:${target.id}`;
}

export function renderForgeDocument(repository: ForgeRepository, artifact: ForgeArtifact): RenderedForgeDocument {
	const lines: string[] = [];
	const fieldSpans: ForgeFieldSpan[] = [];
	const add = (line: string) => {
		lines.push(line);
	};
	const addContent = (indent: string, name: string, value: string, mutable: boolean) => {
		const chunks = fieldChunks(value);
		for (let index = 0; index < chunks.length; index++) {
			const chunk = chunks[index];
			const first = index === 0;
			const last = index === chunks.length - 1;
			const prefix = first
				? `${indent}<${name}${mutable ? ' mutable="true"' : ""}><![CDATA[`
				: `${indent}--><![CDATA[`;
			const suffix = last ? `]]></${name}>` : `]]>${chunk.lineBreak}<!-- forge-break`;
			add(`${prefix}${escapeCdata(chunk.text)}${suffix}`);
		}
		return chunks;
	};
	const addField = (indent: string, name: "title" | "body", value: string, target: ForgeMutationTarget) => {
		const chunks = addContent(indent, name, value, true);
		for (let index = 0; index < chunks.length; index++) {
			const chunk = chunks[index];
			fieldSpans.push({
				line: lines.length - chunks.length + index,
				target,
				field: name,
				start: chunk.start,
				end: chunk.end,
				totalCodeUnits: value.length,
			});
		}
	};
	const addReadonly = (indent: string, name: string, value: string) => {
		addContent(indent, name, value, false);
	};

	add(
		`<forge-artifact${attributes([
			["version", "1"],
			["forge", repository.forge],
			["repository", repository.projectPath],
			["kind", artifact.kind],
			["number", artifact.number],
			["url", artifact.url],
		])}>`,
	);
	add(
		`  <metadata${attributes([
			["state", artifact.state],
			["created-at", artifact.createdAt],
			["updated-at", artifact.updatedAt],
		])}>`,
	);
	add(`    <author${attributes(actorAttributes(artifact.author))}/>`);
	add("    <labels>");
	for (const label of [...artifact.labels].sort(compareText)) addReadonly("      ", "label", label);
	add("    </labels>");
	add("    <assignees>");
	for (const assignee of [...artifact.assignees].sort((left, right) =>
		compareText(left.login ?? "", right.login ?? ""),
	)) {
		add(`      <assignee${attributes(actorAttributes(assignee))}/>`);
	}
	add("    </assignees>");
	add("  </metadata>");

	if (artifact.kind === "issue") {
		add(`  <relationships${attributes([["capability", artifact.relationships.capability]])}>`);
		for (const item of [...artifact.relationships.items].sort((left, right) => {
			const relation = compareText(left.relation, right.relation);
			return relation || left.number - right.number || compareText(left.source, right.source);
		})) {
			add(
				`    <issue${attributes([
					["relation", item.relation],
					["source", item.source],
					["number", item.number],
					["state", item.state],
					["url", item.url],
				])}>`,
			);
			addReadonly("      ", "title", item.title);
			add("    </issue>");
		}
		add("  </relationships>");
	} else {
		add(
			`  <readiness${attributes([
				["draft", artifact.readiness.draft],
				["mergeable", artifact.readiness.mergeable],
				["review-decision", artifact.readiness.reviewDecision],
				["head", artifact.readiness.head],
				["base", artifact.readiness.base],
			])}/>`,
		);
	}

	addField("  ", "title", artifact.title, { kind: "artifact" });
	addField("  ", "body", artifact.body, { kind: "artifact" });
	add("  <comments>");
	for (const comment of [...artifact.comments].sort(
		(left, right) => compareText(left.createdAt, right.createdAt) || compareText(left.id, right.id),
	)) {
		add(
			`    <comment${attributes([
				["id", comment.id],
				["url", comment.url],
				["created-at", comment.createdAt],
				["updated-at", comment.updatedAt],
			])}>`,
		);
		add(`      <author${attributes(actorAttributes(comment.author))}/>`);
		addField("      ", "body", comment.body, { kind: "comment", id: comment.id });
		add("    </comment>");
	}
	add("  </comments>");
	const coreLines = { start: 0, end: lines.length };
	const include: ForgePrSection[] = [];

	if (artifact.kind === "pr") {
		if (artifact.sections.files !== undefined) {
			include.push("files");
			add("  <files>");
			for (const file of [...artifact.sections.files].sort((left, right) => compareText(left.path, right.path))) {
				add(
					`    <file${attributes([
						["status", file.status],
						["additions", file.additions],
						["deletions", file.deletions],
					])}>`,
				);
				addReadonly("      ", "path", file.path);
				if (file.previousPath !== null) addReadonly("      ", "previous-path", file.previousPath);
				add("    </file>");
			}
			add("  </files>");
		}
		if (artifact.sections.commits !== undefined) {
			include.push("commits");
			add("  <commits>");
			for (const commit of [...artifact.sections.commits].sort(
				(left, right) => compareText(left.createdAt, right.createdAt) || compareText(left.sha, right.sha),
			)) {
				add(
					`    <commit${attributes([
						["sha", commit.sha],
						["created-at", commit.createdAt],
						["url", commit.url],
					])}>`,
				);
				addReadonly("      ", "title", commit.title);
				if (commit.author !== null) addReadonly("      ", "author", commit.author);
				add("    </commit>");
			}
			add("  </commits>");
		}
		if (artifact.sections.checks !== undefined) {
			include.push("checks");
			add("  <checks>");
			for (const check of [...artifact.sections.checks].sort(
				(left, right) => compareText(left.name, right.name) || compareText(left.id, right.id),
			)) {
				add(
					`    <check${attributes([
						["id", check.id],
						["status", check.status],
						["conclusion", check.conclusion],
						["url", check.url],
					])}>`,
				);
				addReadonly("      ", "name", check.name);
				add("    </check>");
			}
			add("  </checks>");
		}
	}
	add("</forge-artifact>");

	const text = lines.join("\n");
	return {
		text,
		lines,
		snapshot: createHash("sha256").update(text, "utf8").digest("hex"),
		repository,
		artifact: { kind: artifact.kind, number: artifact.number },
		include,
		fieldSpans,
		coreLines,
	};
}

function mergeRanges(ranges: ForgeCoverageRange[]): ForgeCoverageRange[] {
	const sorted = ranges.filter((range) => range.end > range.start).sort((left, right) => left.start - right.start);
	const merged: ForgeCoverageRange[] = [];
	for (const range of sorted) {
		const previous = merged.at(-1);
		if (previous && range.start <= previous.end) previous.end = Math.max(previous.end, range.end);
		else merged.push({ ...range });
	}
	return merged;
}

function fieldCoverage(rendered: RenderedForgeDocument, start: number, end: number): ForgeFieldCoverage[] {
	const grouped = new Map<
		string,
		{ target: ForgeMutationTarget; field: "title" | "body"; total: number; ranges: ForgeCoverageRange[] }
	>();
	for (const span of rendered.fieldSpans) {
		if (span.line < start || span.line >= end) continue;
		const key = `${targetKey(span.target)}:${span.field}`;
		const group = grouped.get(key) ?? {
			target: span.target,
			field: span.field,
			total: span.totalCodeUnits,
			ranges: [],
		};
		group.ranges.push({ start: span.start, end: span.end });
		grouped.set(key, group);
	}
	return [...grouped.values()].map((group): ForgeFieldCoverage => {
		const coverage = { totalCodeUnits: group.total, ranges: mergeRanges(group.ranges) };
		if (group.target.kind === "comment") {
			if (group.field !== "body") throw new Error("Comment coverage can only describe body content");
			return { ...coverage, target: group.target, field: "body" };
		}
		return { ...coverage, target: { kind: "artifact" }, field: group.field };
	});
}

function positiveInteger(value: number | undefined, name: string): void {
	if (value !== undefined && (!Number.isInteger(value) || value <= 0))
		throw new Error(`${name} must be a positive integer`);
}

export function sliceForgeDocument(rendered: RenderedForgeDocument, request: ForgeRangeRequest): ForgeDocumentSlice {
	positiveInteger(request.offset, "offset");
	positiveInteger(request.limit, "limit");
	if (request.snapshot !== undefined && request.snapshot !== rendered.snapshot) {
		throw new Error("Forge artifact snapshot changed; restart reading from offset=1");
	}
	const start = (request.offset ?? 1) - 1;
	if (start >= rendered.lines.length) {
		throw new Error(
			`Offset ${request.offset} is beyond end of forge document (${rendered.lines.length} lines total)`,
		);
	}
	const requestedEnd = Math.min(rendered.lines.length, start + (request.limit ?? rendered.lines.length));
	const selected = rendered.lines.slice(start, requestedEnd).join("\n");
	const truncation = truncateHead(selected, { maxLines: DEFAULT_MAX_LINES, maxBytes: DEFAULT_MAX_BYTES });
	if (truncation.outputLines === 0) throw new Error("A forge document line exceeds the output byte limit");
	const end = start + truncation.outputLines;
	const hasMore = end < rendered.lines.length;
	const nextOffset = hasMore ? end + 1 : undefined;
	let content = truncation.content;
	if (nextOffset !== undefined) {
		content += `\n\n[Showing lines ${start + 1}-${end} of ${rendered.lines.length}. Use offset=${nextOffset} snapshot=${rendered.snapshot} to continue.]`;
	}
	const coreStart = Math.max(start, rendered.coreLines.start);
	const coreEnd = Math.min(end, rendered.coreLines.end);
	return {
		content,
		details: {
			schema: "scramjet:forge-read@1",
			repository: rendered.repository,
			artifact: rendered.artifact,
			snapshot: rendered.snapshot,
			include: [...rendered.include],
			range: { offset: start + 1, lines: truncation.outputLines, totalLines: rendered.lines.length },
			fields: fieldCoverage(rendered, start, end),
			core: {
				totalLines: rendered.coreLines.end - rendered.coreLines.start,
				ranges: coreEnd > coreStart ? [{ start: coreStart, end: coreEnd }] : [],
			},
		},
		truncated: hasMore,
		...(nextOffset === undefined ? {} : { nextOffset }),
	};
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRange(value: unknown, total: number): value is ForgeCoverageRange {
	return (
		isRecord(value) &&
		Number.isInteger(value.start) &&
		Number.isInteger(value.end) &&
		(value.start as number) >= 0 &&
		(value.end as number) > (value.start as number) &&
		(value.end as number) <= total
	);
}

function isTarget(value: unknown): value is ForgeMutationTarget {
	if (!isRecord(value)) return false;
	if (value.kind === "artifact") return Object.keys(value).length === 1;
	return value.kind === "comment" && typeof value.id === "string" && value.id !== "";
}

export function isForgeReadDetails(value: unknown): value is ForgeReadDetails {
	if (!isRecord(value) || value.schema !== "scramjet:forge-read@1") return false;
	if (!isRecord(value.repository)) return false;
	const repository = value.repository;
	if (
		(repository.forge !== "github" && repository.forge !== "gitlab") ||
		(repository.host !== "github.com" && repository.host !== "gitlab.com") ||
		typeof repository.projectPath !== "string" ||
		repository.projectPath === "" ||
		(repository.forge === "github") !== (repository.host === "github.com")
	) {
		return false;
	}
	if (
		!isRecord(value.artifact) ||
		(value.artifact.kind !== "issue" && value.artifact.kind !== "pr") ||
		!Number.isInteger(value.artifact.number) ||
		(value.artifact.number as number) <= 0 ||
		typeof value.snapshot !== "string" ||
		!/^[a-f0-9]{64}$/.test(value.snapshot)
	) {
		return false;
	}
	if (!Array.isArray(value.include) || value.include.some((item) => !["files", "commits", "checks"].includes(item))) {
		return false;
	}
	if (!isRecord(value.range)) return false;
	const offset = value.range.offset;
	const lines = value.range.lines;
	const totalLines = value.range.totalLines;
	if (
		!Number.isInteger(offset) ||
		!Number.isInteger(lines) ||
		!Number.isInteger(totalLines) ||
		(offset as number) <= 0 ||
		(lines as number) <= 0 ||
		(totalLines as number) <= 0 ||
		(offset as number) + (lines as number) - 1 > (totalLines as number)
	) {
		return false;
	}
	if (!Array.isArray(value.fields)) return false;
	for (const field of value.fields) {
		if (
			!isRecord(field) ||
			!isTarget(field.target) ||
			(field.field !== "title" && field.field !== "body") ||
			(field.target.kind === "comment" && field.field !== "body") ||
			!Number.isInteger(field.totalCodeUnits) ||
			(field.totalCodeUnits as number) < 0 ||
			!Array.isArray(field.ranges) ||
			!field.ranges.every((range) => isRange(range, field.totalCodeUnits as number))
		) {
			return false;
		}
	}
	if (!isRecord(value.core) || !Number.isInteger(value.core.totalLines) || (value.core.totalLines as number) < 0) {
		return false;
	}
	const core = value.core;
	return Array.isArray(core.ranges) && core.ranges.every((range) => isRange(range, core.totalLines as number));
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
	for (const match of ordered.reverse())
		result = result.slice(0, match.start) + match.newText + result.slice(match.end);
	return result;
}
