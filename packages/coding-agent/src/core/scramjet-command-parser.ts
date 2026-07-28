// SCRAMJET-DIVERGENCE: scramjet-command block rendering and restoration (issues 82, 414)

import type { CustomEntry, SessionEntry, SessionMessageEntry } from "./session-manager.js";

export interface ParsedScramjetCommandBlock {
	name: string;
	content: string;
	userMessage?: string;
	userContext?: string;
}

export function parseScramjetCommandBlock(text: string): ParsedScramjetCommandBlock | null {
	const match = text.match(/^<scramjet-command name="([^"]+)">\n([\s\S]*?)\n<\/scramjet-command>(?:\n\n([\s\S]+))?$/);
	if (!match) return null;

	const name = match[1];
	const content = match[2];
	const userMessage = match[3]?.trim() || undefined;

	let userContext: string | undefined;
	const ctxMatch = content.match(/<user-context>\n?([\s\S]*?)\n?<\/user-context>/);
	if (ctxMatch) {
		const extracted = ctxMatch[1].trim();
		if (extracted) userContext = extracted;
	}

	return { name, content, userMessage, userContext };
}

function extractMessageText(entry: SessionMessageEntry): string {
	if (entry.message.role !== "user") return "";
	const { content } = entry.message;
	if (typeof content === "string") return content;

	return content
		.filter((part): part is { type: "text"; text: string } => part.type === "text" && typeof part.text === "string")
		.map((part) => part.text)
		.join("");
}

// exactInvocation hand-mirrors scramjet's CommandStartData / SidebarEntry shape,
// and restoreScramjetCommandInvocation hardcodes the "scramjet:command-start"
// custom-entry type (COMMAND_START_TYPE in packages/scramjet/src/history.ts).
// coding-agent cannot depend on scramjet, so this duplication is structural: if
// the field set or the type literal drifts out of sync, exact restoration
// silently degrades to the semantic fallback with no compile or test signal here.
// A round-trip drift-guard test lives in packages/scramjet/tests/ (which can
// import both sides) to catch that drift. (S3, issue 414)
function exactInvocation(data: unknown, commandName: string): string | null {
	if (typeof data !== "object" || data === null) return null;
	const record = data as Record<string, unknown>;
	if (
		record.command !== commandName ||
		record.depth !== 0 ||
		!(["user", "agent", "forced"] as unknown[]).includes(record.origin) ||
		typeof record.timestamp !== "number" ||
		typeof record.invocationText !== "string"
	) {
		return null;
	}

	const tokenEnd = record.invocationText.search(/\s/);
	const slashToken = tokenEnd === -1 ? record.invocationText : record.invocationText.slice(0, tokenEnd);
	return slashToken === `/${commandName}` ? record.invocationText : null;
}

export function restoreScramjetCommandInvocation(
	selectedEntry: SessionMessageEntry,
	entries: readonly SessionEntry[],
): string {
	const text = extractMessageText(selectedEntry);
	const parsed = parseScramjetCommandBlock(text);
	if (!parsed || !/^[^\s/]+$/.test(parsed.name)) return text;

	const commandStart = entries.find(
		(entry): entry is CustomEntry =>
			entry.type === "custom" &&
			// Mirrors scramjet's COMMAND_START_TYPE (see exactInvocation drift note).
			entry.customType === "scramjet:command-start" &&
			entry.parentId === selectedEntry.id,
	);
	if (commandStart) {
		const invocation = exactInvocation(commandStart.data, parsed.name);
		if (invocation !== null) return invocation;
	}

	let invocation = `/${parsed.name}`;
	if (parsed.userContext) invocation += ` ${parsed.userContext}`;
	if (parsed.userMessage) invocation += `\n\n${parsed.userMessage}`;
	return invocation;
}
