import { createHash } from "node:crypto";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES } from "@leanandmean/coding-agent";
import { FORGE_EXEC_TIMEOUT_MS, type ForgeExec, runForgeCommand, runForgeCommandResult } from "./client.js";
import { type ForgeFilterContext, filterForgeReply } from "./filter.js";
import type {
	ForgeCoverageRange,
	ForgePayloadRange,
	ForgeReadDetails,
	ForgeReadPlan,
	ForgeReadPlanSegment,
	ForgeReadSegmentId,
	ForgeReadSegmentReceipt,
	ForgeRepository,
	ForgeSegmentCoverage,
} from "./types.js";

const NOTICE_RESERVE_BYTES = 768;

interface ItemIndex {
	total: number;
	items: string[];
	render(start: number, end: number): string;
}

export interface PreparedForgeSegment {
	spec: ForgeReadPlanSegment;
	echo: string;
	output: string;
	status: "ok" | "optional_error";
	snapshot?: string;
	index?: ItemIndex;
}

export interface PreparedForgeRead {
	repository: ForgeRepository;
	artifact: ForgeReadPlan["artifact"];
	include: ForgeReadSegmentId[];
	snapshot: string;
	transcript: string;
	segments: PreparedForgeSegment[];
}

export interface ForgeWindowRequest {
	include: ForgeReadSegmentId[];
	offset?: number;
	limit?: number;
	byteOffset?: number;
}

export interface ForgeWindow {
	content: string;
	details: ForgeReadDetails;
	truncated: boolean;
}

function digest(value: string): string {
	return createHash("sha256").update(value, "utf8").digest("hex");
}

function shellQuote(value: string): string {
	return /^[A-Za-z0-9_@%+=:,./-]+$/.test(value) && value !== "" ? value : `'${value.replaceAll("'", `'"'"'`)}'`;
}

export function shellCommand(command: string, args: readonly string[]): string {
	return [command, ...args].map(shellQuote).join(" ");
}

function contextFor(id: ForgeReadSegmentId): ForgeFilterContext {
	return id;
}

function atPath(value: unknown, path: readonly string[]): unknown[] {
	let current = value;
	for (const key of path) {
		if (typeof current !== "object" || current === null || Array.isArray(current)) {
			throw new Error(`Forge native ${key} reply was malformed`);
		}
		current = (current as Record<string, unknown>)[key];
	}
	if (!Array.isArray(current)) throw new Error("Forge native list reply was malformed");
	return current;
}

function replacePath(
	value: Record<string, unknown>,
	path: readonly string[],
	items: unknown[],
): Record<string, unknown> {
	if (path.length === 0) throw new Error("Forge native list path was empty");
	const clone = { ...value };
	let current = clone;
	for (const [index, key] of path.entries()) {
		if (index === path.length - 1) {
			current[key] = items;
			break;
		}
		const next = current[key];
		if (typeof next !== "object" || next === null || Array.isArray(next)) {
			throw new Error("Forge native list reply was malformed");
		}
		current[key] = { ...(next as Record<string, unknown>) };
		current = current[key] as Record<string, unknown>;
	}
	return clone;
}

function jsonItemIndex(output: string): ItemIndex {
	JSON.parse(output);
	return { total: 1, items: [output], render: () => output };
}

function ndjsonItemIndex(output: string): ItemIndex {
	const items = output === "" ? [] : output.split("\n");
	for (const item of items) JSON.parse(item);
	return {
		total: items.length,
		items,
		render: (start, end) => items.slice(start, end).join("\n"),
	};
}

function ghSlurpItemIndex(output: string, path: readonly string[] = []): ItemIndex {
	const root = JSON.parse(output);
	if (!Array.isArray(root)) throw new Error("Forge native gh --slurp reply was malformed");
	const locations: Array<{ page: number; item: number }> = [];
	const items: string[] = [];
	for (const [pageIndex, rawPage] of root.entries()) {
		const pageItems = path.length === 0 ? rawPage : atPath(rawPage, path);
		if (!Array.isArray(pageItems)) throw new Error("Forge native gh --slurp page was malformed");
		for (const [itemIndex, item] of pageItems.entries()) {
			locations.push({ page: pageIndex, item: itemIndex });
			items.push(JSON.stringify(item));
		}
	}
	return {
		total: items.length,
		items,
		render(start, end) {
			if (start === 0 && end === items.length) return output;
			const selected = new Map<number, unknown[]>();
			for (let index = start; index < end; index++) {
				const location = locations[index];
				const page = root[location.page];
				const pageItems = path.length === 0 ? page : atPath(page, path);
				const group = selected.get(location.page) ?? [];
				group.push(pageItems[location.item]);
				selected.set(location.page, group);
			}
			const pages = [...selected].map(([pageIndex, pageItems]) => {
				if (path.length === 0) return pageItems;
				const page = root[pageIndex];
				if (typeof page !== "object" || page === null || Array.isArray(page)) {
					throw new Error("Forge native gh --slurp envelope was malformed");
				}
				return replacePath(page as Record<string, unknown>, path, pageItems);
			});
			return JSON.stringify(pages);
		},
	};
}

function itemIndex(segment: ForgeReadPlanSegment, output: string): ItemIndex {
	if (segment.shape.kind === "json") return jsonItemIndex(output);
	if (segment.shape.kind === "ndjson") return ndjsonItemIndex(output);
	return ghSlurpItemIndex(output, segment.shape.itemsPath);
}

export async function executeForgeReadPlan(
	plan: ForgeReadPlan,
	exec: ForgeExec,
	cwd: string,
	signal?: AbortSignal,
): Promise<PreparedForgeRead> {
	const segments: PreparedForgeSegment[] = [];
	for (const spec of plan.segments) {
		const invocation = {
			command: spec.command,
			args: spec.args,
			cwd,
			signal,
			timeout: FORGE_EXEC_TIMEOUT_MS,
		};
		const result = spec.optional
			? await runForgeCommandResult(exec, invocation)
			: await runForgeCommand(exec, invocation);
		const echo = `$ ${shellCommand(spec.command, spec.args)}`;
		if (result.code !== 0) {
			segments.push({ spec, echo, output: result.stderr, status: "optional_error" });
			continue;
		}
		let output: string;
		let index: ItemIndex;
		try {
			output = filterForgeReply(
				result.stdout,
				plan.repository.forge,
				contextFor(spec.id),
				spec.shape.kind === "ndjson",
			);
			index = itemIndex(spec, output);
		} catch (error) {
			throw new Error(
				`Forge segment ${spec.id} from ${shellCommand(spec.command, spec.args)} did not match its ${spec.shape.kind} reply contract: ${error instanceof Error ? error.message : String(error)}`,
			);
		}
		segments.push({ spec, echo, output, status: "ok", snapshot: digest(output), index });
	}
	const transcript = segments.map((segment) => `${segment.echo}\n${segment.output}\n`).join("");
	return {
		repository: plan.repository,
		artifact: plan.artifact,
		include: [...plan.include],
		snapshot: digest(transcript),
		transcript,
		segments,
	};
}

function positiveInteger(value: number | undefined, name: string): void {
	if (value !== undefined && (!Number.isInteger(value) || value <= 0)) {
		throw new Error(`${name} must be a positive integer`);
	}
}

function byteRange(start: number, text: string): ForgePayloadRange {
	return { start, end: start + Buffer.byteLength(text, "utf8") };
}

function appendSegment(
	content: string,
	segment: PreparedForgeSegment,
	output: string,
	coverage: ForgeSegmentCoverage | undefined,
	showCommand: boolean,
): { content: string; receipt: ForgeReadSegmentReceipt } {
	const start = Buffer.byteLength(content, "utf8");
	const commandText = showCommand ? `${segment.echo}\n` : "";
	const segmentText = `${commandText}${output}\n`;
	const command = showCommand ? byteRange(start, segment.echo) : undefined;
	const outputRange = byteRange(start + Buffer.byteLength(commandText, "utf8"), output);
	return {
		content: content + segmentText,
		receipt: {
			id: segment.spec.id,
			status: segment.status,
			...(segment.snapshot === undefined ? {} : { snapshot: segment.snapshot }),
			...(segment.spec.evidence === undefined ? {} : { evidence: segment.spec.evidence }),
			...(coverage === undefined ? {} : { coverage }),
			payload: {
				segment: byteRange(start, segmentText),
				...(command === undefined ? {} : { command }),
				output: outputRange,
			},
		},
	};
}

function utf8Boundary(buffer: Buffer, offset: number): boolean {
	return offset === 0 || offset === buffer.length || (buffer[offset] & 0xc0) !== 0x80;
}

function byteFragment(value: string, offset: number, maxBytes: number): { text: string; bytes: number; total: number } {
	const buffer = Buffer.from(value, "utf8");
	const start = offset - 1;
	if (start < 0 || start >= buffer.length || !utf8Boundary(buffer, start)) {
		throw new Error("byte_offset is outside the oversized JSON item or splits UTF-8");
	}
	let end = Math.min(buffer.length, start + maxBytes);
	while (end > start && !utf8Boundary(buffer, end)) end--;
	if (end === start) throw new Error("Output byte limit cannot fit one UTF-8 code point");
	return { text: buffer.subarray(start, end).toString("utf8"), bytes: end - start, total: buffer.length };
}

function continuationNotice(
	segment: PreparedForgeSegment,
	shown: string,
	include: readonly ForgeReadSegmentId[],
	offset: number,
	snapshot: string,
	byteOffset?: number,
): string {
	return `[showing ${segment.spec.id} ${shown}; continue with include=${JSON.stringify(include)} offset=${offset}${
		byteOffset === undefined ? "" : ` byte_offset=${byteOffset}`
	} and unchanged snapshot=${snapshot}]`;
}

function segmentFor(read: PreparedForgeRead, id: ForgeReadSegmentId): PreparedForgeSegment {
	const segment = read.segments.find((candidate) => candidate.spec.id === id);
	if (segment === undefined)
		throw new Error(`Snapshot does not contain forge segment ${id}; restart without snapshot`);
	return segment;
}

export function windowForgeRead(read: PreparedForgeRead, request: ForgeWindowRequest): ForgeWindow {
	positiveInteger(request.offset, "offset");
	positiveInteger(request.limit, "limit");
	positiveInteger(request.byteOffset, "byte_offset");
	if (request.include.length === 0) throw new Error("include must select at least one forge segment");
	const selected = request.include.map((id) => segmentFor(read, id));
	let content = "";
	const receipts: ForgeReadSegmentReceipt[] = [];
	let notice = "";
	let truncated = false;
	const maxItems = Math.min(request.limit ?? DEFAULT_MAX_LINES, DEFAULT_MAX_LINES);

	for (const [segmentIndex, segment] of selected.entries()) {
		const offset = segmentIndex === 0 ? (request.offset ?? 1) : 1;
		const byteOffset = segmentIndex === 0 ? request.byteOffset : undefined;
		const remainingInclude = request.include.slice(segmentIndex);
		const available = DEFAULT_MAX_BYTES - Buffer.byteLength(content, "utf8") - NOTICE_RESERVE_BYTES;
		if (available <= 0) {
			notice = continuationNotice(segment, "not yet shown", remainingInclude, offset, read.snapshot, byteOffset);
			truncated = true;
			break;
		}
		if (segment.status === "optional_error") {
			const rendered = `${segment.echo}\n${segment.output}\n`;
			if (Buffer.byteLength(rendered, "utf8") > available) {
				throw new Error(`Optional forge segment ${segment.spec.id} exceeds the output byte limit`);
			}
			const appended = appendSegment(content, segment, segment.output, undefined, true);
			content = appended.content;
			receipts.push(appended.receipt);
			continue;
		}
		const index = segment.index as ItemIndex;
		if (offset > Math.max(index.total, 1)) {
			throw new Error(`Offset ${offset} is beyond forge segment ${segment.spec.id} (${index.total} items total)`);
		}
		if (byteOffset !== undefined) {
			if (index.total === 0 || offset > index.total) throw new Error("byte_offset requires an existing JSON item");
			const commandBytes = offset === 1 && byteOffset === 1 ? Buffer.byteLength(`${segment.echo}\n`, "utf8") : 0;
			if (content !== "" && available - commandBytes - 1 < 4) {
				notice = continuationNotice(segment, "not yet shown", remainingInclude, offset, read.snapshot, byteOffset);
				truncated = true;
				break;
			}
			const fragment = byteFragment(index.items[offset - 1], byteOffset, available - commandBytes - 1);
			const coverage: ForgeSegmentCoverage = {
				unit: "bytes",
				item: offset,
				offset: byteOffset,
				bytes: fragment.bytes,
				totalBytes: fragment.total,
				totalItems: index.total,
			};
			const appended = appendSegment(content, segment, fragment.text, coverage, offset === 1 && byteOffset === 1);
			content = appended.content;
			receipts.push(appended.receipt);
			const nextByte = byteOffset + fragment.bytes;
			if (nextByte <= fragment.total) {
				notice = continuationNotice(
					segment,
					`bytes ${byteOffset}-${nextByte - 1} of item ${offset} (${fragment.total} bytes)`,
					remainingInclude,
					offset,
					read.snapshot,
					nextByte,
				);
				truncated = true;
			} else if (offset < index.total) {
				notice = continuationNotice(
					segment,
					`bytes ${byteOffset}-${nextByte - 1} of item ${offset} (${fragment.total} bytes)`,
					remainingInclude,
					offset + 1,
					read.snapshot,
				);
				truncated = true;
			} else if (segmentIndex < selected.length - 1) {
				const nextSegment = selected[segmentIndex + 1];
				notice = continuationNotice(
					nextSegment,
					"not yet shown",
					request.include.slice(segmentIndex + 1),
					1,
					read.snapshot,
				);
				truncated = true;
			}
			break;
		}

		const start = offset - 1;
		if (index.total === 0) {
			const output = index.render(0, 0);
			const appended = appendSegment(
				content,
				segment,
				output,
				{ unit: "items", offset: 1, count: 0, totalItems: 0 },
				true,
			);
			content = appended.content;
			receipts.push(appended.receipt);
			continue;
		}
		let end = start;
		const upper = Math.min(index.total, start + maxItems);
		while (end < upper) {
			const candidate = index.render(start, end + 1);
			const commandBytes = start === 0 ? Buffer.byteLength(`${segment.echo}\n`, "utf8") : 0;
			if (Buffer.byteLength(candidate, "utf8") + commandBytes + 1 > available) break;
			end++;
		}
		if (end === start) {
			const commandBytes = start === 0 ? Buffer.byteLength(`${segment.echo}\n`, "utf8") : 0;
			if (content !== "") {
				notice = continuationNotice(segment, "not yet shown", remainingInclude, offset, read.snapshot);
				truncated = true;
				break;
			}
			const fragment = byteFragment(index.items[start], 1, available - commandBytes - 1);
			const appended = appendSegment(
				content,
				segment,
				fragment.text,
				{
					unit: "bytes",
					item: offset,
					offset: 1,
					bytes: fragment.bytes,
					totalBytes: fragment.total,
					totalItems: index.total,
				},
				start === 0,
			);
			content = appended.content;
			receipts.push(appended.receipt);
			notice = continuationNotice(
				segment,
				`bytes 1-${fragment.bytes} of item ${offset} (${fragment.total} bytes)`,
				remainingInclude,
				offset,
				read.snapshot,
				fragment.bytes + 1,
			);
			truncated = true;
			break;
		}
		const output = index.render(start, end);
		const appended = appendSegment(
			content,
			segment,
			output,
			{ unit: "items", offset, count: end - start, totalItems: index.total },
			start === 0,
		);
		content = appended.content;
		receipts.push(appended.receipt);
		if (end < index.total) {
			notice = continuationNotice(
				segment,
				`items ${offset}-${end} of ${index.total}`,
				remainingInclude,
				end + 1,
				read.snapshot,
			);
			truncated = true;
			break;
		}
	}

	if (notice !== "") content += `${content === "" ? "" : "\n"}${notice}`;
	if (Buffer.byteLength(content, "utf8") > DEFAULT_MAX_BYTES)
		throw new Error("Forge read exceeded the output byte limit");
	return {
		content,
		details: {
			schema: "scramjet:forge-read@2",
			repository: read.repository,
			artifact: read.artifact,
			snapshot: read.snapshot,
			include: [...request.include],
			segments: receipts,
		},
		truncated,
	};
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

const SEGMENTS = new Set<ForgeReadSegmentId>([
	"artifact",
	"comments",
	"sub_issues",
	"parent",
	"relationships",
	"files",
	"commits",
	"check_runs",
	"status",
	"pipelines",
]);

function isRange(value: unknown): value is ForgePayloadRange {
	return (
		isRecord(value) &&
		Number.isInteger(value.start) &&
		Number.isInteger(value.end) &&
		(value.start as number) >= 0 &&
		(value.end as number) >= (value.start as number)
	);
}

function isCoverage(value: unknown): value is ForgeSegmentCoverage {
	if (!isRecord(value)) return false;
	if (value.unit === "items") {
		return (
			Number.isInteger(value.offset) &&
			(value.offset as number) >= 1 &&
			Number.isInteger(value.count) &&
			(value.count as number) >= 0 &&
			Number.isInteger(value.totalItems) &&
			(value.totalItems as number) >= 0 &&
			(value.offset as number) - 1 + (value.count as number) <= (value.totalItems as number)
		);
	}
	return (
		value.unit === "bytes" &&
		Number.isInteger(value.item) &&
		(value.item as number) >= 1 &&
		Number.isInteger(value.offset) &&
		(value.offset as number) >= 1 &&
		Number.isInteger(value.bytes) &&
		(value.bytes as number) > 0 &&
		Number.isInteger(value.totalBytes) &&
		(value.totalBytes as number) > 0 &&
		(value.offset as number) - 1 + (value.bytes as number) <= (value.totalBytes as number) &&
		Number.isInteger(value.totalItems) &&
		(value.totalItems as number) >= (value.item as number)
	);
}

export function isForgeReadDetails(value: unknown): value is ForgeReadDetails {
	if (!isRecord(value) || value.schema !== "scramjet:forge-read@2") return false;
	if (!isRecord(value.repository) || !isRecord(value.artifact)) return false;
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
		(value.artifact.kind !== "issue" && value.artifact.kind !== "pr") ||
		!Number.isInteger(value.artifact.number) ||
		(value.artifact.number as number) <= 0 ||
		typeof value.snapshot !== "string" ||
		!/^[a-f0-9]{64}$/.test(value.snapshot) ||
		!Array.isArray(value.include) ||
		value.include.length === 0 ||
		value.include.some((id) => !SEGMENTS.has(id as ForgeReadSegmentId)) ||
		new Set(value.include).size !== value.include.length ||
		!Array.isArray(value.segments) ||
		value.segments.length === 0
	) {
		return false;
	}
	const include = value.include as ForgeReadSegmentId[];
	const seen = new Set<ForgeReadSegmentId>();
	let previousEnd = 0;
	for (const [segmentIndex, segment] of value.segments.entries()) {
		if (!isRecord(segment) || !SEGMENTS.has(segment.id as ForgeReadSegmentId)) return false;
		const id = segment.id as ForgeReadSegmentId;
		if (include[segmentIndex] !== id || seen.has(id)) return false;
		seen.add(id);
		if (segment.status !== "ok" && segment.status !== "optional_error") return false;
		if (!isRecord(segment.payload) || !isRange(segment.payload.segment) || !isRange(segment.payload.output)) {
			return false;
		}
		const payload = segment.payload;
		const segmentRange = payload.segment as ForgePayloadRange;
		const outputRange = payload.output as ForgePayloadRange;
		if (
			segmentRange.end <= segmentRange.start ||
			segmentRange.start !== previousEnd ||
			outputRange.start < segmentRange.start ||
			outputRange.end + 1 !== segmentRange.end
		) {
			return false;
		}
		previousEnd = segmentRange.end;
		if (payload.command !== undefined) {
			if (!isRange(payload.command)) return false;
			const commandRange = payload.command as ForgePayloadRange;
			if (commandRange.start !== segmentRange.start || commandRange.end + 1 !== outputRange.start) return false;
		} else if (outputRange.start !== segmentRange.start) {
			return false;
		}
		if (segment.status === "optional_error") {
			if (
				id !== "parent" ||
				segment.snapshot !== undefined ||
				segment.coverage !== undefined ||
				segment.evidence !== undefined
			) {
				return false;
			}
			continue;
		}
		if (typeof segment.snapshot !== "string" || !/^[a-f0-9]{64}$/.test(segment.snapshot)) return false;
		if (segment.evidence !== undefined && segment.evidence !== "artifact" && segment.evidence !== "comments") {
			return false;
		}
		if ((segment.evidence === "artifact") !== (id === "artifact")) return false;
		if ((segment.evidence === "comments") !== (id === "comments")) return false;
		if (!isCoverage(segment.coverage)) return false;
		if (id === "artifact" && segment.coverage.totalItems !== 1) return false;
	}
	return true;
}

function payloadSlice(content: string, range: ForgePayloadRange): string | null {
	const buffer = Buffer.from(content, "utf8");
	if (range.end > buffer.length) return null;
	const slice = buffer.subarray(range.start, range.end);
	const decoded = slice.toString("utf8");
	return Buffer.from(decoded, "utf8").equals(slice) ? decoded : null;
}

export function isForgeReadPayload(content: string, details: unknown): details is ForgeReadDetails {
	if (!isForgeReadDetails(details)) return false;
	for (const segment of details.segments) {
		const segmentText = payloadSlice(content, segment.payload.segment);
		const output = payloadSlice(content, segment.payload.output);
		if (segmentText === null || output === null) return false;
		let expected = `${output}\n`;
		if (segment.payload.command !== undefined) {
			const command = payloadSlice(content, segment.payload.command);
			if (command === null || !command.startsWith("$ ")) return false;
			expected = `${command}\n${expected}`;
		}
		if (segmentText !== expected) return false;
	}
	return true;
}

function mergeRanges(ranges: ForgeCoverageRange[]): ForgeCoverageRange[] {
	const sorted = [...ranges].sort((left, right) => left.start - right.start);
	const merged: ForgeCoverageRange[] = [];
	for (const range of sorted) {
		const previous = merged.at(-1);
		if (previous !== undefined && range.start <= previous.end) previous.end = Math.max(previous.end, range.end);
		else merged.push({ ...range });
	}
	return merged;
}

export function hasCompleteSegmentCoverage(receipts: readonly ForgeReadSegmentReceipt[]): boolean {
	const successful = receipts.filter(
		(receipt): receipt is ForgeReadSegmentReceipt & { snapshot: string; coverage: ForgeSegmentCoverage } =>
			receipt.status === "ok" && receipt.snapshot !== undefined && receipt.coverage !== undefined,
	);
	if (successful.length === 0) return false;
	const totalItems = successful[0].coverage.totalItems;
	if (successful.some((receipt) => receipt.coverage.totalItems !== totalItems)) return false;
	if (totalItems === 0)
		return successful.some(
			(receipt) =>
				receipt.coverage.unit === "items" && receipt.coverage.offset === 1 && receipt.coverage.count === 0,
		);
	const itemRanges: ForgeCoverageRange[] = [];
	const bytes = new Map<number, { total: number; ranges: ForgeCoverageRange[] }>();
	for (const receipt of successful) {
		const coverage = receipt.coverage;
		if (coverage.unit === "items") {
			if (coverage.count > 0) itemRanges.push({ start: coverage.offset, end: coverage.offset + coverage.count });
			continue;
		}
		const group = bytes.get(coverage.item) ?? { total: coverage.totalBytes, ranges: [] };
		if (group.total !== coverage.totalBytes) return false;
		group.ranges.push({ start: coverage.offset, end: coverage.offset + coverage.bytes });
		bytes.set(coverage.item, group);
	}
	for (const [item, group] of bytes) {
		const merged = mergeRanges(group.ranges);
		if (merged.length === 1 && merged[0].start === 1 && merged[0].end === group.total + 1) {
			itemRanges.push({ start: item, end: item + 1 });
		}
	}
	const merged = mergeRanges(itemRanges);
	return merged.length === 1 && merged[0].start === 1 && merged[0].end === totalItems + 1;
}
