import { getMarkdownTheme, type Theme, type ToolRenderResultOptions } from "@leanandmean/coding-agent";
import { Container, Markdown, Spacer, Text } from "@leanandmean/tui";
import { isForgeReadDetails, isForgeReadPayload } from "./native-reply.js";
import { controlSafeText, losslessControlSafeText } from "./text.js";
import type { ForgeReadDetails, ForgeReadSegmentId } from "./types.js";

interface BodyBlock {
	label: string;
	body: string;
}

interface ReadableSegment {
	id: ForgeReadSegmentId;
	command?: string;
	metadata?: string;
	bodies: BodyBlock[];
	error?: string;
}

interface ReadableReply {
	segments: ReadableSegment[];
	continuation?: string;
}

function byteSlice(value: string, start: number, end: number): string | null {
	const buffer = Buffer.from(value, "utf8");
	if (start < 0 || end < start || end > buffer.length) return null;
	const slice = buffer.subarray(start, end);
	const decoded = slice.toString("utf8");
	return Buffer.from(decoded, "utf8").equals(slice) ? decoded : null;
}

function record(value: unknown): Record<string, unknown> | null {
	return typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: null;
}

function withoutBody(
	value: unknown,
	field: "body" | "description",
	label: string,
	bodies: BodyBlock[],
): Record<string, unknown> {
	const source = record(value);
	if (source === null) throw new Error("Forge body-bearing reply was malformed");
	const body = source[field];
	if (body === undefined || body === null) return source;
	if (typeof body !== "string") throw new Error("Forge body field was malformed");
	const metadata = { ...source };
	delete metadata[field];
	bodies.push({ label, body });
	return metadata;
}

function githubMetadata(id: ForgeReadSegmentId, output: string, bodies: BodyBlock[]): unknown {
	const parsed = JSON.parse(output);
	if (id === "artifact") return withoutBody(parsed, "body", "Body", bodies);
	if (id === "parent") {
		if (record(parsed) === null) throw new Error("GitHub parent reply was malformed");
		return parsed;
	}
	if (!Array.isArray(parsed)) throw new Error("GitHub list reply was malformed");
	if (id === "check_runs" || id === "status") {
		const field = id === "check_runs" ? "check_runs" : "statuses";
		for (const page of parsed) {
			const envelope = record(page);
			if (envelope === null || !Array.isArray(envelope[field])) {
				throw new Error(`GitHub ${id} envelope was malformed`);
			}
		}
		return parsed;
	}
	for (const page of parsed) if (!Array.isArray(page)) throw new Error("GitHub list page was malformed");
	if (id !== "comments") return parsed;
	let commentNumber = 0;
	return parsed.map((page) =>
		(page as unknown[]).map((comment) => {
			commentNumber++;
			return withoutBody(comment, "body", `Comment ${commentNumber} body`, bodies);
		}),
	);
}

function gitlabValues(output: string): Record<string, unknown>[] {
	if (output === "") return [];
	return output.split("\n").map((line) => {
		const value = JSON.parse(line);
		const item = record(value);
		if (item === null) throw new Error("GitLab NDJSON record was malformed");
		return item;
	});
}

function gitlabMetadata(id: ForgeReadSegmentId, output: string, bodies: BodyBlock[]): unknown {
	if (id === "artifact") return withoutBody(JSON.parse(output), "description", "Description", bodies);
	const values = gitlabValues(output);
	if (id !== "comments") return values;
	return values.map((comment, index) => withoutBody(comment, "body", `Comment ${index + 1} body`, bodies));
}

function readableReply(content: string, details: ForgeReadDetails): ReadableReply | null {
	if (!isForgeReadPayload(content, details)) return null;
	if (details.segments.some((segment) => segment.coverage?.unit === "bytes")) return null;
	try {
		const segments: ReadableSegment[] = [];
		for (const segment of details.segments) {
			const command =
				segment.payload.command === undefined
					? undefined
					: byteSlice(content, segment.payload.command.start, segment.payload.command.end);
			const output = byteSlice(content, segment.payload.output.start, segment.payload.output.end);
			if (command === null || output === null) return null;
			if (segment.status === "optional_error") {
				segments.push({ id: segment.id, ...(command === undefined ? {} : { command }), bodies: [], error: output });
				continue;
			}
			const bodies: BodyBlock[] = [];
			const metadata =
				details.repository.forge === "github"
					? githubMetadata(segment.id, output, bodies)
					: gitlabMetadata(segment.id, output, bodies);
			segments.push({
				id: segment.id,
				...(command === undefined ? {} : { command }),
				metadata: JSON.stringify(metadata, null, 2),
				bodies,
			});
		}
		const lastEnd = details.segments.at(-1)?.payload.segment.end ?? 0;
		const continuation = byteSlice(content, lastEnd, Buffer.byteLength(content, "utf8"));
		if (continuation === null) return null;
		return { segments, ...(continuation.trim() === "" ? {} : { continuation }) };
	} catch {
		return null;
	}
}

function summary(details: ForgeReadDetails): string {
	return details.segments
		.map((segment) => {
			const coverage = segment.coverage;
			if (coverage?.unit === "items")
				return `${segment.id} ${coverage.offset}-${coverage.offset + Math.max(coverage.count - 1, 0)}/${coverage.totalItems}`;
			if (coverage?.unit === "bytes")
				return `${segment.id} item ${coverage.item} bytes ${coverage.offset}-${coverage.offset + coverage.bytes - 1}`;
			return `${segment.id} error`;
		})
		.join(", ");
}

export class ForgeReplyComponent extends Container {
	private content = "";
	private details: unknown;
	private options: ToolRenderResultOptions = { expanded: false, isPartial: false };
	private theme?: Theme;

	update(content: string, details: unknown, options: ToolRenderResultOptions, theme: Theme): void {
		this.content = content;
		this.details = details;
		this.options = options;
		this.theme = theme;
		this.rebuild();
	}

	override invalidate(): void {
		super.invalidate();
		this.rebuild();
	}

	private label(value: string): Text {
		const theme = this.theme as Theme;
		return new Text(theme.fg("toolTitle", theme.bold(value)), 0, 0);
	}

	private output(value: string): Text {
		const theme = this.theme as Theme;
		return new Text(theme.fg("toolOutput", controlSafeText(value)), 0, 0);
	}

	private rebuild(): void {
		this.clear();
		if (this.theme === undefined) return;
		if (this.options.isPartial) {
			this.addChild(new Text(this.theme.fg("warning", "Reading forge artifact..."), 0, 0));
			return;
		}
		if (!this.options.expanded) {
			const collapsed = isForgeReadDetails(this.details) ? summary(this.details) : "forge read result unavailable";
			this.addChild(
				new Text(this.theme.fg(isForgeReadDetails(this.details) ? "success" : "warning", collapsed), 0, 0),
			);
			return;
		}
		if (!isForgeReadDetails(this.details)) {
			this.addChild(new Text(losslessControlSafeText(this.content), 0, 0));
			return;
		}

		this.addChild(this.label("Raw transcript (scroll up; reversible escaped display)"));
		this.addChild(new Text(this.theme.fg("toolOutput", losslessControlSafeText(this.content)), 0, 0));

		const readable = readableReply(this.content, this.details);
		if (readable === null) return;
		this.addChild(new Spacer(1));
		this.addChild(this.label("Readable view"));
		for (const segment of readable.segments) {
			this.addChild(new Spacer(1));
			this.addChild(this.label(segment.id));
			if (segment.command !== undefined) this.addChild(this.output(segment.command));
			if (segment.error !== undefined) {
				this.addChild(this.label("Provider error"));
				this.addChild(this.output(segment.error));
				continue;
			}
			this.addChild(this.label("Metadata"));
			this.addChild(this.output(segment.metadata ?? "null"));
			for (const body of segment.bodies) {
				this.addChild(this.label(body.label));
				if (body.body === "") this.addChild(new Text(this.theme.fg("muted", "(empty body)"), 0, 0));
				else this.addChild(new Markdown(controlSafeText(body.body), 0, 0, getMarkdownTheme()));
			}
		}
		if (readable.continuation !== undefined) {
			this.addChild(new Spacer(1));
			this.addChild(this.label("Continuation"));
			this.addChild(this.output(readable.continuation));
		}
	}
}

export function rawForgeReply(content: string): string {
	return controlSafeText(content);
}
