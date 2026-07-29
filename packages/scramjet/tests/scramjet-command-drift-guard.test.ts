import type { SessionEntry } from "@leanandmean/coding-agent";
import { describe, expect, it } from "vitest";
// Deep import into coding-agent's source (not its public API): this drift guard
// exists to couple the two sides, and coding-agent does not re-export the parser.
import { restoreScramjetCommandInvocation } from "../../coding-agent/src/core/scramjet-command-parser.js";
import { COMMAND_START_TYPE, type CommandStartData } from "../src/history.js";

// Drift guard for the cross-package invocation shape (S3, issue 414).
// coding-agent's exactInvocation hand-mirrors CommandStartData and hardcodes the
// COMMAND_START_TYPE literal. This round-trips a real CommandStartData built from
// scramjet's own type and constant through restoreScramjetCommandInvocation: if
// either the field set or the type literal drifts out of sync, exact restoration
// falls back to the semantic form and this assertion fails.
describe("scramjet-command exact-invocation drift guard", () => {
	it("restores the exact invocation text from a real CommandStartData", () => {
		const commandName = "mach12:issue-plan";
		const invocationText = "/mach12:issue-plan 42 focus on edge cases";
		const data: CommandStartData = {
			command: commandName,
			origin: "user",
			depth: 0,
			timestamp: 1234,
			invocationText,
		};
		const messageId = "msg-1";
		const messageEntry = {
			type: "message",
			id: messageId,
			parentId: null,
			timestamp: "0",
			message: {
				role: "user",
				content: `<scramjet-command name="${commandName}">\nbody\n</scramjet-command>`,
				timestamp: 0,
			},
		} as SessionEntry;
		const startEntry = {
			type: "custom",
			customType: COMMAND_START_TYPE,
			data,
			id: "start-1",
			parentId: messageId,
			timestamp: "0",
		} as SessionEntry;

		expect(restoreScramjetCommandInvocation(messageEntry as never, [messageEntry, startEntry])).toBe(invocationText);
	});
});
