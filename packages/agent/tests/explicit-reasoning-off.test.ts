import {
	type AssistantMessage,
	createAssistantMessageEventStream,
	type Model,
	type SimpleStreamOptions,
	streamSimple,
} from "@leanandmean/ai";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AgentHarness } from "../src/harness/agent-harness.js";
import { NodeExecutionEnv } from "../src/harness/env/nodejs.js";
import { InMemorySessionRepo } from "../src/harness/session/memory-repo.js";

vi.mock("@leanandmean/ai", async (importOriginal) => {
	const actual = await importOriginal<typeof import("@leanandmean/ai")>();
	return { ...actual, streamSimple: vi.fn() };
});

const model: Model<"openai-responses"> = {
	id: "gpt-6-sol",
	name: "GPT-6 Sol",
	api: "openai-responses",
	provider: "github-copilot",
	baseUrl: "https://api.individual.githubcopilot.com",
	reasoning: true,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
	contextWindow: 1000000,
	maxTokens: 128000,
	thinkingLevelMap: { off: "none" },
};

function completedMessage(): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "text", text: "done" }],
		api: model.api,
		provider: model.provider,
		model: model.id,
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0 },
		stopReason: "stop",
		timestamp: Date.now(),
	};
}

describe("AgentHarness explicit reasoning off", () => {
	beforeEach(() => {
		vi.mocked(streamSimple).mockReset();
	});

	it.each([
		["an omitted thinking level", undefined, undefined],
		["an explicit off selection", "off", true],
	] as const)("distinguishes %s", async (_label, thinkingLevel, expectedExplicitOff) => {
		let receivedOptions: SimpleStreamOptions | undefined;
		vi.mocked(streamSimple).mockImplementation((_model, _context, options) => {
			receivedOptions = options;
			const message = completedMessage();
			const stream = createAssistantMessageEventStream();
			stream.push({ type: "start", partial: message });
			stream.push({ type: "done", reason: "stop", message });
			return stream;
		});
		const session = await new InMemorySessionRepo().create();
		const harness = new AgentHarness({
			env: new NodeExecutionEnv({ cwd: process.cwd() }),
			session,
			model,
			thinkingLevel,
			getApiKeyAndHeaders: async () => ({ apiKey: "test-key" }),
		});

		await harness.prompt("hello");

		expect(receivedOptions?.reasoning).toBeUndefined();
		expect(receivedOptions?.explicitReasoningOff).toBe(expectedExplicitOff);
	});
});
