import { buildRegistry, type FileEntry } from "./commands/loader.js";
import type { CommandRegistry } from "./types.js";

export interface CommandLintDiagnostic {
	code: string;
	severity: "error" | "warning";
	filePath: string;
	line?: number;
	message: string;
}

export interface CommandLintResult {
	registry: CommandRegistry;
	diagnostics: CommandLintDiagnostic[];
}

interface MarkdownLine {
	line: number;
	text: string;
}

const GOALS_HEADING = /^ {0,3}##[ \t]+Goals(?:[ \t]+#+)?[ \t]*$/;
const H1_OR_H2 = /^ {0,3}#{1,2}(?:[ \t]+|$)/;
const LIST_ITEM = /^ {0,3}(?:[-+*]|\d+[.)])[ \t]+/;
const ARGUMENT_SUBSTITUTION = /\$(?:ARGUMENTS|@|\d+|\{@:\d+(?::\d+)?\})/g;
const CONTEXT_TAG = /<\/?(?:user|caller)-context>/g;
const PROCEDURAL_H2 =
	/^ {0,3}##[ \t]+(?:Steps?\b|Actions?\b|Process\b|Procedure\b|Workflow\b|Method\b|Implementation\b|Verification\b|Review\b|Summary\b)/i;

function markdownLines(content: string): MarkdownLine[] {
	const lines = content.split(/\r?\n/);
	if (lines[0]?.charCodeAt(0) === 0xfeff) lines[0] = lines[0].slice(1);
	let start = 0;
	if (lines[0] === "---") {
		const closing = lines.findIndex((line, index) => index > 0 && line === "---");
		if (closing !== -1) start = closing + 1;
	}

	const visible: MarkdownLine[] = [];
	let fence: { marker: "`" | "~"; length: number } | undefined;
	for (let index = start; index < lines.length; index++) {
		const text = lines[index];
		if (fence) {
			const closing = new RegExp(`^ {0,3}\\${fence.marker}{${fence.length},}[ \\t]*$`);
			if (closing.test(text)) fence = undefined;
			continue;
		}
		const opening = text.match(/^ {0,3}(`{3,}|~{3,})/);
		if (opening) {
			const marker = opening[1];
			fence = { marker: marker[0] as "`" | "~", length: marker.length };
			continue;
		}
		visible.push({ line: index + 1, text });
	}
	return visible;
}

function warning(entry: FileEntry, code: string, message: string, line?: number): CommandLintDiagnostic {
	return { code, severity: "warning", filePath: entry.filePath, ...(line === undefined ? {} : { line }), message };
}

function goalsDiagnostics(entry: FileEntry, lines: MarkdownLine[]): CommandLintDiagnostic[] {
	const headings = lines.filter((line) => GOALS_HEADING.test(line.text));
	if (headings.length === 0) {
		return [warning(entry, "goals-missing", 'Add an early "## Goals" section describing durable outcomes.')];
	}

	const diagnostics: CommandLintDiagnostic[] = [];
	for (const duplicate of headings.slice(1)) {
		diagnostics.push(warning(entry, "goals-duplicate", 'Keep exactly one "## Goals" section.', duplicate.line));
	}

	for (const goals of headings) {
		const sectionEnd =
			lines.find((line) => line.line > goals.line && H1_OR_H2.test(line.text))?.line ?? Number.POSITIVE_INFINITY;
		const section = lines.filter((line) => line.line > goals.line && line.line < sectionEnd);
		const content = section.filter((line) => line.text.trim() !== "");
		if (content.length === 0) {
			diagnostics.push(warning(entry, "goals-empty", 'Add durable outcomes beneath "## Goals".', goals.line));
		}
		const earlierProcedure = lines.find((line) => line.line < goals.line && PROCEDURAL_H2.test(line.text));
		if (earlierProcedure) {
			diagnostics.push(warning(entry, "goals-order", 'Move "## Goals" before procedural sections.', goals.line));
		}
		if (content.length > 0 && !content.some((line) => LIST_ITEM.test(line.text))) {
			diagnostics.push(warning(entry, "goals-list", "Express Goals as Markdown list items.", goals.line));
		}
		const embeddedContext = section.find((line) => {
			ARGUMENT_SUBSTITUTION.lastIndex = 0;
			CONTEXT_TAG.lastIndex = 0;
			return ARGUMENT_SUBSTITUTION.test(line.text) || CONTEXT_TAG.test(line.text);
		});
		if (embeddedContext) {
			diagnostics.push(
				warning(
					entry,
					"goals-context",
					"Keep argument substitution and user/caller context outside Goals.",
					embeddedContext.line,
				),
			);
		}
	}
	return diagnostics;
}

function framingDiagnostic(
	entry: FileEntry,
	lines: MarkdownLine[],
	delegateOnly: boolean,
	acceptsArguments: boolean,
): CommandLintDiagnostic | undefined {
	const text = lines.map((line) => line.text).join("\n");
	const expected = delegateOnly ? "caller-context" : "user-context";
	const unexpected = delegateOnly ? "user-context" : "caller-context";
	const opening = `<${expected}>`;
	const closing = `</${expected}>`;
	ARGUMENT_SUBSTITUTION.lastIndex = 0;
	const substitutions = [...text.matchAll(ARGUMENT_SUBSTITUTION)];
	const hasArgumentFraming =
		acceptsArguments ||
		substitutions.length > 0 ||
		text.includes(opening) ||
		text.includes(closing) ||
		text.includes(`<${unexpected}>`) ||
		text.includes(`</${unexpected}>`);
	if (!hasArgumentFraming) return undefined;

	const openingCount = text.split(opening).length - 1;
	const closingCount = text.split(closing).length - 1;
	const blockStart = text.indexOf(opening);
	const blockEnd = text.indexOf(closing, blockStart + opening.length);
	const substitutionInsideBlock =
		blockStart !== -1 &&
		blockEnd !== -1 &&
		substitutions.length === 1 &&
		substitutions[0].index !== undefined &&
		substitutions[0].index >= blockStart + opening.length &&
		substitutions[0].index < blockEnd;
	const valid =
		openingCount === 1 &&
		closingCount === 1 &&
		!text.includes(`<${unexpected}>`) &&
		!text.includes(`</${unexpected}>`) &&
		substitutionInsideBlock;
	if (valid) return undefined;

	const relevantLine = lines.find((line) => {
		ARGUMENT_SUBSTITUTION.lastIndex = 0;
		return line.text.includes("-context>") || ARGUMENT_SUBSTITUTION.test(line.text);
	});
	return warning(
		entry,
		"context-framing",
		`Use exactly one argument substitution inside one <${expected}> block${delegateOnly ? " for this delegate-only command" : " for this top-level command"}.`,
		relevantLine?.line,
	);
}

export function lintCommandEntries(entries: FileEntry[]): CommandLintResult {
	const { registry, outcomes } = buildRegistry(entries);
	const diagnostics: CommandLintDiagnostic[] = [];
	for (const outcome of outcomes) {
		if (outcome.kind === "unrecognized") {
			diagnostics.push({
				code: "runtime-unrecognized",
				severity: "error",
				filePath: outcome.entry.filePath,
				message: outcome.error,
			});
		} else {
			if (outcome.kind === "shadowed") {
				diagnostics.push({
					code: "runtime-shadowed",
					severity: "error",
					filePath: outcome.entry.filePath,
					message: `Command ${outcome.def.name} is shadowed by ${outcome.registeredFrom.filePath}.`,
				});
			}
			for (const notice of outcome.notices) {
				diagnostics.push({
					code: "runtime-notice",
					severity: "warning",
					filePath: outcome.entry.filePath,
					message: notice,
				});
			}
		}

		const lines = markdownLines(outcome.entry.content);
		diagnostics.push(...goalsDiagnostics(outcome.entry, lines));
		if (outcome.kind !== "unrecognized") {
			const framing = framingDiagnostic(
				outcome.entry,
				lines,
				outcome.def.delegateOnly === true,
				outcome.def.argumentHint !== undefined,
			);
			if (framing) diagnostics.push(framing);
		}
	}
	return { registry, diagnostics };
}
