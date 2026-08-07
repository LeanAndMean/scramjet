import { createHash } from "node:crypto";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES } from "@leanandmean/coding-agent";
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
const COMPACT_RECORD_MAX_BYTES = 2048;

export interface ForgeFieldSpan {
	line: number;
	target: ForgeMutationTarget;
	field: "title" | "body";
	start: number;
	end: number;
	totalCodeUnits: number;
}

export interface RenderedForgeDocument {
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
	breakAfter: "none" | "line" | "forced";
}

function compareText(left: string, right: string): number {
	return left < right ? -1 : left > right ? 1 : 0;
}

function forgeEscape(codeUnit: number): string {
	return `^!${codeUnit.toString(16).toUpperCase().padStart(4, "0")};`;
}

function presentationUnsafe(codeUnit: number): boolean {
	return (
		(codeUnit >= 0x7f && codeUnit <= 0x9f) ||
		codeUnit === 0x061c ||
		codeUnit === 0x200e ||
		codeUnit === 0x200f ||
		(codeUnit >= 0x202a && codeUnit <= 0x202e) ||
		(codeUnit >= 0x2066 && codeUnit <= 0x2069)
	);
}

function escapeAttribute(value: string | number | boolean): string {
	const text = String(value);
	let output = "";
	for (let index = 0; index < text.length; index++) {
		const codeUnit = text.charCodeAt(index);
		if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
			const next = text.charCodeAt(index + 1);
			if (next >= 0xdc00 && next <= 0xdfff) {
				output += text.slice(index, index + 2);
				index++;
				continue;
			}
		}
		if (
			codeUnit === 0x22 ||
			codeUnit === 0x5e ||
			codeUnit < 0x20 ||
			presentationUnsafe(codeUnit) ||
			codeUnit === 0xfffe ||
			codeUnit === 0xffff ||
			(codeUnit >= 0xd800 && codeUnit <= 0xdfff)
		) {
			output += forgeEscape(codeUnit);
			continue;
		}
		output += text[index];
	}
	return output;
}

function attributes(values: Array<[string, string | number | boolean | null | undefined]>): string {
	return values
		.filter((entry): entry is [string, string | number | boolean] => entry[1] !== null && entry[1] !== undefined)
		.map(([name, value]) => ` ${name}="${escapeAttribute(value)}"`)
		.join("");
}

function actorAttributes(actor: ForgeActor, prefix = ""): Array<[string, string | null]> {
	if (prefix === "author-") {
		return [
			["author", actor.login],
			["author-kind", actor.kind === "user" ? null : actor.kind],
		];
	}
	return [
		[`${prefix}login`, actor.login],
		[`${prefix}kind`, actor.kind],
	];
}

function openDirective(
	name: string,
	values: Array<[string, string | number | boolean | null | undefined]> = [],
): string {
	return `^${name}${attributes(values)}{`;
}

function closeDirective(name: string): string {
	return `^${name}}`;
}

function emptyDirective(
	name: string,
	values: Array<[string, string | number | boolean | null | undefined]> = [],
): string {
	return `^${name}${attributes(values)};`;
}

function compactRecord(name: string, values: Array<[string, string | number | boolean | null]>): string | null {
	const line = emptyDirective(name, values);
	return Buffer.byteLength(line, "utf8") <= COMPACT_RECORD_MAX_BYTES ? line : null;
}

function escapeText(value: string): string {
	let output = "";
	for (let index = 0; index < value.length; index++) {
		const codeUnit = value.charCodeAt(index);
		if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
			const next = value.charCodeAt(index + 1);
			if (next >= 0xdc00 && next <= 0xdfff) {
				output += value.slice(index, index + 2);
				index++;
				continue;
			}
		}
		if (codeUnit === 0x0a) {
			output += "\n";
			continue;
		}
		if (
			codeUnit === 0x5e ||
			codeUnit < 0x20 ||
			presentationUnsafe(codeUnit) ||
			codeUnit === 0xfffe ||
			codeUnit === 0xffff ||
			(codeUnit >= 0xd800 && codeUnit <= 0xdfff)
		) {
			output += forgeEscape(codeUnit);
			continue;
		}
		output += value[index];
	}
	return output;
}

function fieldChunks(value: string): FieldChunk[] {
	const chunks: FieldChunk[] = [];
	let chunkStart = 0;
	let chunkText = "";
	let chunkBytes = 0;
	let index = 0;

	const push = (end: number, breakAfter: FieldChunk["breakAfter"]) => {
		chunks.push({ text: chunkText, start: chunkStart, end, breakAfter });
		chunkText = "";
		chunkBytes = 0;
		chunkStart = end;
	};

	while (index < value.length) {
		const char = String.fromCodePoint(value.codePointAt(index) as number);
		if (char === "\n") {
			index++;
			push(index, "line");
			continue;
		}
		const charBytes = Buffer.byteLength(char, "utf8");
		if (chunkText !== "" && chunkBytes + charBytes > FIELD_CHUNK_BYTES) push(index, "forced");
		chunkText += char;
		chunkBytes += charBytes;
		index += char.length;
	}
	push(value.length, "none");
	return chunks;
}

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

function targetKey(target: ForgeMutationTarget): string {
	return target.kind === "artifact" ? "artifact" : `comment:${target.id}`;
}

export function renderForgeDocument(repository: ForgeRepository, artifact: ForgeArtifact): RenderedForgeDocument {
	const lines: string[] = [];
	const fieldSpans: ForgeFieldSpan[] = [];
	const add = (line: string, breakBefore = true) => {
		if (lines.length > 0 && breakBefore) lines[lines.length - 1] += "\n";
		lines.push(line);
	};
	const addContent = (indent: string, name: string, value: string) => {
		const chunks = fieldChunks(value);
		let precedingBreak: FieldChunk["breakAfter"] = "none";
		for (let index = 0; index < chunks.length; index++) {
			const chunk = chunks[index];
			const first = index === 0;
			const last = index === chunks.length - 1;
			const prefix = first ? `${indent}${openDirective(name)}` : "";
			const suffix = last ? closeDirective(name) : "";
			add(`${prefix}${escapeText(chunk.text)}${suffix}`, precedingBreak !== "forced");
			precedingBreak = chunk.breakAfter;
		}
		return chunks;
	};
	const addField = (indent: string, name: "title" | "body", value: string, target: ForgeMutationTarget) => {
		const chunks = addContent(indent, name, value);
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
		addContent(indent, name, value);
	};

	add(
		openDirective("artifact", [
			["format", "forge-caret-1"],
			["content-trust", "untrusted"],
			["repository", repository.projectPath],
			["kind", artifact.kind],
			["number", artifact.number],
			["url", artifact.url],
			["state", artifact.state],
			["created-at", artifact.createdAt],
			["updated-at", artifact.updatedAt],
			...actorAttributes(artifact.author, "author-"),
		]),
	);
	for (const label of [...artifact.labels].sort(compareText)) addReadonly("  ", "label", label);
	for (const assignee of [...artifact.assignees].sort((left, right) =>
		compareText(left.login ?? "", right.login ?? ""),
	)) {
		add(`  ${emptyDirective("assignee", actorAttributes(assignee))}`);
	}

	if (artifact.kind === "issue") {
		const capability = artifact.relationships.capability === "supported" ? null : artifact.relationships.capability;
		if (artifact.relationships.items.length === 0) {
			add(`  ${emptyDirective("relationships", [["capability", capability]])}`);
		} else {
			add(`  ${openDirective("relationships", [["capability", capability]])}`);
			for (const item of [...artifact.relationships.items].sort((left, right) => {
				const relation = compareText(left.relation, right.relation);
				return (
					relation ||
					compareText(left.repository.projectPath, right.repository.projectPath) ||
					left.number - right.number ||
					compareText(left.source, right.source)
				);
			})) {
				add(
					`    ${openDirective("issue", [
						["relation", item.relation],
						["source", item.source],
						["repository", item.repository.projectPath],
						["number", item.number],
						["state", item.state],
						["url", item.url],
					])}`,
				);
				addReadonly("      ", "title", item.title);
				add(`    ${closeDirective("issue")}`);
			}
			add(`  ${closeDirective("relationships")}`);
		}
	} else {
		add(
			`  ${emptyDirective("readiness", [
				["draft", artifact.readiness.draft],
				["mergeable", artifact.readiness.mergeable],
				["review-decision-capability", artifact.readiness.reviewDecision.capability],
				[
					"review-decision",
					artifact.readiness.reviewDecision.capability === "unsupported"
						? null
						: artifact.readiness.reviewDecision.value,
				],
				["head", artifact.readiness.head],
				["base", artifact.readiness.base],
			])}`,
		);
	}

	addField("  ", "title", artifact.title, { kind: "artifact" });
	addField("  ", "body", artifact.body, { kind: "artifact" });
	if (artifact.comments.length > 0) add(`  ${openDirective("comments")}`);
	for (const comment of [...artifact.comments].sort(
		(left, right) => compareText(left.createdAt, right.createdAt) || compareText(left.id, right.id),
	)) {
		add(
			`    ${openDirective("comment", [
				["id", comment.id],
				["url", comment.url],
				["created-at", comment.createdAt],
				["updated-at", comment.updatedAt === comment.createdAt ? null : comment.updatedAt],
				...actorAttributes(comment.author, "author-"),
			])}`,
		);
		addField("      ", "body", comment.body, { kind: "comment", id: comment.id });
		add(`    ${closeDirective("comment")}`);
	}
	if (artifact.comments.length > 0) add(`  ${closeDirective("comments")}`);
	const coreLines = { start: 0, end: lines.length };
	const include: ForgePrSection[] = [];

	if (artifact.kind === "pr") {
		if (artifact.sections.files !== undefined) {
			include.push("files");
			add(`  ${openDirective("files")}`);
			for (const file of [...artifact.sections.files].sort((left, right) => compareText(left.path, right.path))) {
				const compact = compactRecord("file", [
					["path", file.path],
					["previous-path", file.previousPath],
					["status", file.status],
					["additions", file.additions],
					["deletions", file.deletions],
				]);
				if (compact !== null) add(compact);
				else {
					add(
						`    ${openDirective("file", [
							["additions", file.additions],
							["deletions", file.deletions],
						])}`,
					);
					addReadonly("      ", "path", file.path);
					if (file.previousPath !== null) addReadonly("      ", "previous-path", file.previousPath);
					addReadonly("      ", "status", file.status);
					add(`    ${closeDirective("file")}`);
				}
			}
			add(`  ${closeDirective("files")}`);
		}
		if (artifact.sections.commits !== undefined) {
			include.push("commits");
			add(`  ${openDirective("commits")}`);
			for (const commit of [...artifact.sections.commits].sort(
				(left, right) => compareText(left.createdAt, right.createdAt) || compareText(left.sha, right.sha),
			)) {
				const compact = compactRecord("commit", [
					["sha", commit.sha],
					["title", commit.title],
					["author", commit.author],
					["created-at", commit.createdAt],
					["url", commit.url],
				]);
				if (compact !== null) add(compact);
				else {
					add(`    ${openDirective("commit")}`);
					addReadonly("      ", "sha", commit.sha);
					addReadonly("      ", "title", commit.title);
					if (commit.author !== null) addReadonly("      ", "author", commit.author);
					addReadonly("      ", "created-at", commit.createdAt);
					if (commit.url !== null) addReadonly("      ", "url", commit.url);
					add(`    ${closeDirective("commit")}`);
				}
			}
			add(`  ${closeDirective("commits")}`);
		}
		if (artifact.sections.checks !== undefined) {
			include.push("checks");
			add(`  ${openDirective("checks")}`);
			for (const check of [...artifact.sections.checks].sort(
				(left, right) => compareText(left.name, right.name) || compareText(left.id, right.id),
			)) {
				const compact = compactRecord("check", [
					["id", check.id],
					["name", check.name],
					["status", check.status],
					["conclusion", check.conclusion],
					["url", check.url],
				]);
				if (compact !== null) add(compact);
				else {
					add(`    ${openDirective("check")}`);
					addReadonly("      ", "id", check.id);
					addReadonly("      ", "name", check.name);
					addReadonly("      ", "status", check.status);
					if (check.conclusion !== null) addReadonly("      ", "conclusion", check.conclusion);
					if (check.url !== null) addReadonly("      ", "url", check.url);
					add(`    ${closeDirective("check")}`);
				}
			}
			add(`  ${closeDirective("checks")}`);
		}
	}
	add(closeDirective("artifact"));

	const text = lines.join("");
	return {
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
			`Offset ${request.offset} is beyond end of forge document (${rendered.lines.length} positions total)`,
		);
	}
	const requestedEnd = Math.min(
		rendered.lines.length,
		start + Math.min(request.limit ?? DEFAULT_MAX_LINES, DEFAULT_MAX_LINES),
	);
	let end = start;
	let selectedBytes = 0;
	while (end < requestedEnd) {
		const segmentBytes = Buffer.byteLength(rendered.lines[end], "utf8");
		if (selectedBytes + segmentBytes > DEFAULT_MAX_BYTES) break;
		selectedBytes += segmentBytes;
		end++;
	}
	if (end === start) throw new Error("A forge document position exceeds the output byte limit");
	const include = rendered.include.length === 0 ? null : rendered.include.join(",");
	let continuation = "";
	while (end < rendered.lines.length) {
		const nextOffset = end + 1;
		continuation = emptyDirective("continue", [
			["shown", `${start + 1}-${end}`],
			["total", rendered.lines.length],
			["next-offset", nextOffset],
			["snapshot", rendered.snapshot],
			["include", include],
		]);
		if (selectedBytes + Buffer.byteLength(`\n\n${continuation}`, "utf8") <= DEFAULT_MAX_BYTES) break;
		end--;
		selectedBytes -= Buffer.byteLength(rendered.lines[end], "utf8");
		if (end === start) throw new Error("A forge document position exceeds the output byte limit");
	}
	const hasMore = end < rendered.lines.length;
	const nextOffset = hasMore ? end + 1 : undefined;
	let content = rendered.lines.slice(start, end).join("");
	if (hasMore) content += `\n\n${continuation}`;
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
			range: { offset: start + 1, lines: end - start, totalLines: rendered.lines.length },
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
