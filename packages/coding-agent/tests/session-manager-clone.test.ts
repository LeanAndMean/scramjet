/**
 * Issue 361 (S3): `AgentSessionRuntime.fork()`'s in-memory branch clones the live SessionManager via
 * `cloneInMemory()` before branching, so a failed candidate never corrupts the live session. That safety
 * rests on an invariant the types do not enforce: `newSession()` / `createBranchedSession()` must *reassign*
 * `fileEntries` (and build fresh index state) rather than mutate the shared array or its entry objects in
 * place. `cloneInMemory()` only shallow-copies `fileEntries`, so an in-place mutation on the clone would leak
 * straight back into the source — reintroducing the exact live-manager corruption issue 361 fixed.
 *
 * These unit tests pin that invariant at the SessionManager layer (the runtime fork tests only exercise it
 * indirectly): after applying each mutator to a clone, the source's session id, leaf, and every entry object
 * (by deep value) are untouched.
 */

import type { AssistantMessage, UserMessage } from "@leanandmean/ai";
import { describe, expect, it } from "vitest";
import { SessionManager } from "../src/core/session-manager.js";

function userMessage(text: string): UserMessage {
	return { role: "user", content: text, timestamp: Date.now() };
}

function assistantText(text: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text }],
		api: "openai-chat",
		provider: "provider-a",
		model: "model-a",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

function makeSource(): SessionManager {
	const source = SessionManager.inMemory("/clone-independence-cwd");
	source.appendMessage(userMessage("hello"));
	source.appendMessage(assistantText("world"));
	return source;
}

describe("SessionManager.cloneInMemory() independence", () => {
	it("newSession() on a clone leaves the source's id, leaf, and entry objects untouched", () => {
		const source = makeSource();
		const sourceId = source.getSessionId();
		const sourceLeaf = source.getLeafId();
		const sourceEntries = structuredClone(source.getEntries());

		const clone = source.cloneInMemory();
		clone.newSession();

		expect(source.getSessionId()).toBe(sourceId);
		expect(source.getLeafId()).toBe(sourceLeaf);
		// Deep equality catches both array-content changes and in-place mutation of shared entry objects.
		expect(source.getEntries()).toEqual(sourceEntries);
	});

	it("createBranchedSession() on a clone leaves the source's id, leaf, and entry objects untouched", () => {
		const source = makeSource();
		const leafId = source.getLeafId();
		expect(leafId).not.toBeNull();
		const sourceId = source.getSessionId();
		const sourceEntries = structuredClone(source.getEntries());

		const clone = source.cloneInMemory();
		clone.createBranchedSession(leafId as string);

		expect(source.getSessionId()).toBe(sourceId);
		expect(source.getLeafId()).toBe(leafId);
		expect(source.getEntries()).toEqual(sourceEntries);
	});
});
