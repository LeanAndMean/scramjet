import type { AgentMessage } from "@leanandmean/agent";
import { describe, expect, it } from "vitest";
import { restoreScramjetCommandInvocation } from "../src/core/scramjet-command-parser.js";
import type { CustomEntry, SessionEntry, SessionMessageEntry } from "../src/core/session-manager.js";

function expanded(name = "mach12:issue-plan", context?: string, trailing?: string): string {
	const contextBlock = context === undefined ? "" : `\n<user-context>\n${context}\n</user-context>\n`;
	return `<scramjet-command name="${name}">\n# Command\n${contextBlock}</scramjet-command>${trailing ? `\n\n${trailing}` : ""}`;
}

function messageEntry(id: string, text: string, parentId: string | null = null): SessionMessageEntry {
	return {
		type: "message",
		id,
		parentId,
		timestamp: "2026-01-01T00:00:00.000Z",
		message: { role: "user", content: text, timestamp: 0 } as AgentMessage,
	};
}

function commandStart(parentId: string, data: unknown): CustomEntry {
	return {
		type: "custom",
		customType: "scramjet:command-start",
		id: `${parentId}-start`,
		parentId,
		timestamp: "2026-01-01T00:00:00.001Z",
		data,
	};
}

const validCommandStartData = {
	command: "mach12:issue-plan",
	origin: "user",
	depth: 0,
	timestamp: 1,
	invocationText: "/mach12:issue-plan exact",
};

describe("restoreScramjetCommandInvocation", () => {
	it("restores exact invocation text from directly correlated metadata", () => {
		const selected = messageEntry("message-1", expanded());
		const invocationText = '/mach12:issue-plan\t 55  "quoted value"\nsecond line';

		expect(
			restoreScramjetCommandInvocation(selected, [
				selected,
				commandStart(selected.id, {
					command: "mach12:issue-plan",
					origin: "user",
					depth: 0,
					timestamp: 1,
					invocationText,
				}),
			]),
		).toBe(invocationText);
	});

	it("isolates identical expanded messages on sibling branches", () => {
		const first = messageEntry("first", expanded(), "root");
		const second = messageEntry("second", expanded(), "root");
		const entries: SessionEntry[] = [
			first,
			commandStart(first.id, {
				command: "mach12:issue-plan",
				origin: "user",
				depth: 0,
				timestamp: 1,
				invocationText: "/mach12:issue-plan first",
			}),
			second,
			commandStart(second.id, {
				command: "mach12:issue-plan",
				origin: "user",
				depth: 0,
				timestamp: 2,
				invocationText: "/mach12:issue-plan second",
			}),
		];

		expect(restoreScramjetCommandInvocation(first, entries)).toBe("/mach12:issue-plan first");
		expect(restoreScramjetCommandInvocation(second, entries)).toBe("/mach12:issue-plan second");
	});

	it.each([
		["wrong command", { ...validCommandStartData, command: "mach12:push", invocationText: "/mach12:push exact" }],
		["delegated depth", { ...validCommandStartData, origin: "agent", depth: 1 }],
		["wrong slash token", { ...validCommandStartData, invocationText: "/mach12:issue-plan-extra" }],
		["non-string invocation", { ...validCommandStartData, invocationText: 42 }],
		["malformed payload", null],
	])("rejects %s metadata and uses semantic restoration", (_label, data) => {
		const selected = messageEntry("selected", expanded("mach12:issue-plan", "55"));
		expect(restoreScramjetCommandInvocation(selected, [selected, commandStart(selected.id, data)])).toBe(
			"/mach12:issue-plan 55",
		);
	});

	it("does not borrow orphan or indirectly related metadata", () => {
		const selected = messageEntry("selected", expanded("mach12:issue-plan", "55"));
		const orphan = commandStart("other-message", {
			command: "mach12:issue-plan",
			origin: "user",
			depth: 0,
			invocationText: "/mach12:issue-plan stale",
		});
		expect(restoreScramjetCommandInvocation(selected, [orphan, selected])).toBe("/mach12:issue-plan 55");
	});

	it.each([
		[expanded("mach12:push"), "/mach12:push"],
		[expanded("mach12:issue-plan", "  55\n next  "), "/mach12:issue-plan 55\n next"],
		[expanded("mach12:issue-plan", "55", "additional text"), "/mach12:issue-plan 55\n\nadditional text"],
	])("semantically reconstructs legacy command blocks", (text, expected) => {
		const selected = messageEntry("legacy", text);
		expect(restoreScramjetCommandInvocation(selected, [selected])).toBe(expected);
	});

	it.each([
		"ordinary text",
		'<scramjet-command name="broken">',
		"<scramjet-command></scramjet-command>",
		expanded("mach12:issue-plan 55"),
	])("leaves ordinary or malformed text unchanged", (text) => {
		const selected = messageEntry("plain", text);
		expect(restoreScramjetCommandInvocation(selected, [selected])).toBe(text);
	});
});
