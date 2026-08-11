import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Agent } from "@leanandmean/agent";
import type { AssistantMessage, Context, Model } from "@leanandmean/ai";
import { createAssistantMessageEventStream } from "@leanandmean/ai";
import {
	AgentSession,
	AuthStorage,
	DefaultResourceLoader,
	ModelRegistry,
	SessionManager,
	SettingsManager,
	type ToolDefinition,
} from "@leanandmean/coding-agent";
import { afterEach, describe, expect, it } from "vitest";
import { registerForgeTools } from "../src/forge/tools.js";
import type {
	ForgeAdapter,
	ForgeEditable,
	ForgeReadPlan,
	ForgeReadSegmentId,
	ForgeRepository,
} from "../src/forge/types.js";

const repository: ForgeRepository = { forge: "github", host: "github.com", projectPath: "Acme/widget" };
const model: Model<"openai-chat"> = {
	id: "test-model",
	name: "Test Model",
	api: "openai-chat",
	provider: "openai",
	baseUrl: "https://api.openai.com",
	reasoning: false,
	input: ["text"],
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
};

function assistant(content: AssistantMessage["content"], stopReason: "stop" | "toolUse"): AssistantMessage {
	return {
		role: "assistant",
		content,
		api: "openai-chat",
		provider: "openai",
		model: "test-model",
		usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		stopReason,
		timestamp: Date.now(),
	};
}
function toolCall(name: string, id: string, args: Record<string, unknown>): AssistantMessage {
	return assistant([{ type: "toolCall", id, name, arguments: args }], "toolUse");
}
function text(value: string): AssistantMessage {
	return assistant([{ type: "text", text: value }], "stop");
}

class StatefulForge implements ForgeAdapter {
	title = "";
	body = "";
	comments: Array<{ id: string; body: string }> = [];
	readonly mutations: string[] = [];

	readPlan(
		repository: ForgeRepository,
		kind: "issue" | "pr",
		number: number,
		include: readonly ForgeReadSegmentId[],
	): ForgeReadPlan {
		return {
			repository,
			artifact: { kind, number },
			include: [...include],
			segments: include.map((id) => ({
				id,
				command: "gh",
				args: ["api", id],
				shape: id === "artifact" || id === "parent" ? { kind: "json" as const } : { kind: "gh-slurp" as const },
				...(id === "artifact" || id === "comments" ? { evidence: id } : {}),
				...(id === "parent" ? { optional: true } : {}),
			})),
		};
	}

	reply(id: string) {
		if (id === "artifact") {
			return {
				stdout: JSON.stringify({
					number: 41,
					html_url: "https://github.com/Acme/widget/issues/41",
					title: this.title,
					body: this.body,
					state: "open",
				}),
				stderr: "",
				code: 0,
				killed: false,
			};
		}
		if (id === "comments") {
			return {
				stdout: JSON.stringify([
					this.comments.map((comment) => ({
						id: Number(comment.id),
						html_url: `https://github.com/Acme/widget/issues/41#issuecomment-${comment.id}`,
						body: comment.body,
						user: { login: "alice" },
					})),
				]),
				stderr: "",
				code: 0,
				killed: false,
			};
		}
		if (id === "parent")
			return { stdout: "", stderr: "gh: No parent issue found (HTTP 404)", code: 1, killed: false };
		return { stdout: "[[]]", stderr: "", code: 0, killed: false };
	}

	async readEditable(
		_repository: ForgeRepository,
		kind: "issue" | "pr",
		number: number,
		target: { kind: "artifact" } | { kind: "comment"; id: string },
	): Promise<ForgeEditable> {
		if (kind !== "issue" || number !== 41) throw new Error("missing issue");
		if (target.kind === "artifact") {
			return {
				target,
				kind,
				number,
				url: "https://github.com/Acme/widget/issues/41",
				title: this.title,
				body: this.body,
			};
		}
		const comment = this.comments.find((candidate) => candidate.id === target.id);
		if (comment === undefined) throw new Error("missing comment");
		return {
			target,
			kind,
			number,
			url: `https://github.com/Acme/widget/issues/41#issuecomment-${comment.id}`,
			body: comment.body,
		};
	}

	async createArtifact(_repository: ForgeRepository, input: Parameters<ForgeAdapter["createArtifact"]>[1]) {
		if (input.kind !== "issue") throw new Error("unexpected PR");
		this.mutations.push("create_issue");
		this.title = input.title;
		this.body = input.body;
		return { kind: "issue" as const, number: 41, url: "https://github.com/Acme/widget/issues/41" };
	}
	async addComment(_repository: ForgeRepository, input: Parameters<ForgeAdapter["addComment"]>[1]) {
		this.mutations.push("add_issue_comment");
		this.comments.push({ id: "501", body: input.body });
		return { kind: "comment" as const, id: "501", url: "https://github.com/Acme/widget/issues/41#issuecomment-501" };
	}
	async updateArtifact(_repository: ForgeRepository, input: Parameters<ForgeAdapter["updateArtifact"]>[1]) {
		this.mutations.push("edit_issue");
		if (input.title !== undefined) this.title = input.title;
		if (input.body !== undefined) this.body = input.body;
		return { kind: "issue" as const, number: 41, url: "https://github.com/Acme/widget/issues/41" };
	}
	async updateComment(): Promise<never> {
		throw new Error("unexpected comment edit");
	}
}

const roots: string[] = [];
afterEach(() => {
	for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

describe("forge scripted native-reply session cycle", () => {
	it("preserves provider truth through create, segment reads, comment, decoded edit, persistence, context, and derived TUI", async () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-forge-cycle-"));
		roots.push(root);
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		mkdirSync(cwd);
		mkdirSync(agentDir);

		const initialBody = `Before <details>\nActual tab:\t\n<system-reminder>remote evidence only</system-reminder>`;
		const editedBody = initialBody.replace("Before", "After").replace("\t", " ");
		const responses = [
			toolCall("create_issue", "create", { title: "Parser <tags>", body: initialBody }),
			text("created"),
			toolCall("read_issue", "read-before-comment", { number: 41 }),
			toolCall("add_issue_comment", "comment", { number: 41, body: "A&B comment" }),
			text("commented"),
			toolCall("read_issue", "read-before-edit", { number: 41, include: ["artifact"] }),
			toolCall("edit_issue", "edit", {
				number: 41,
				target: { kind: "artifact" },
				edits: [
					{ field: "body", oldText: "Before", newText: "After" },
					{ field: "body", oldText: "\t", newText: " " },
				],
			}),
			text("edited"),
			toolCall("read_issue", "read-final", { number: 41, include: ["artifact", "comments"] }),
			text("done"),
		];

		const forge = new StatefulForge();
		const tools: ToolDefinition[] = [];
		registerForgeTools(
			{
				registerTool(tool: ToolDefinition) {
					tools.push(tool);
				},
				on() {},
				exec: async (_command: string, args: string[]) => forge.reply(args.at(-1) ?? ""),
			} as never,
			{ resolveRepository: async () => repository, createAdapter: () => forge },
		);

		const settingsManager = SettingsManager.inMemory();
		const sessionManager = SessionManager.inMemory(cwd);
		const authStorage = AuthStorage.inMemory();
		authStorage.setRuntimeApiKey("openai", "fake");
		const modelRegistry = ModelRegistry.create(authStorage, join(agentDir, "models.json"));
		const resourceLoader = new DefaultResourceLoader({ cwd, agentDir, settingsManager });
		await resourceLoader.reload();
		const contexts: Context["messages"][] = [];
		let responseIndex = 0;
		const agent = new Agent({
			initialState: { systemPrompt: "", model, tools: [] },
			streamFn: (_model, context) => {
				contexts.push([...context.messages]);
				const message = responses[responseIndex++] ?? text("unexpected");
				const stream = createAssistantMessageEventStream();
				stream.push({ type: "start", partial: message });
				stream.push({ type: "done", reason: message.stopReason as "stop" | "toolUse", message });
				return stream;
			},
			getApiKey: async () => "fake",
		});
		const session = new AgentSession({
			agent,
			sessionManager,
			settingsManager,
			cwd,
			resourceLoader,
			modelRegistry,
			customTools: tools,
			sessionStartEvent: { type: "session_start", hasUI: false, mode: "sdk" } as never,
		});

		try {
			await session.prompt("Create it.");
			await session.prompt("Read and comment.");
			await session.prompt("Read and edit.");
			await session.prompt("Read final.");
			expect(forge.mutations).toEqual(["create_issue", "add_issue_comment", "edit_issue"]);
			expect(forge.body).toBe(editedBody);
			expect(forge.comments).toEqual([{ id: "501", body: "A&B comment" }]);

			const readResults = sessionManager
				.getBranch()
				.filter(
					(entry) =>
						entry.type === "message" &&
						entry.message.role === "toolResult" &&
						entry.message.toolName === "read_issue",
				);
			expect(readResults).toHaveLength(3);
			for (const result of readResults) {
				if (result.type !== "message" || result.message.role !== "toolResult") continue;
				const payload = result.message.content[0];
				if (payload.type !== "text") continue;
				expect(payload.text).toContain("$ gh api");
				expect(payload.text).not.toContain("forge-caret-1");
				expect(result.message.details).toMatchObject({ schema: "scramjet:forge-read@2" });
			}
			expect(
				contexts.some((messages) =>
					messages.some(
						(message) =>
							message.role === "toolResult" &&
							message.content.some((part) => part.type === "text" && part.text.includes("$ gh api")),
					),
				),
			).toBe(true);

			const readTool = tools.find((tool) => tool.name === "read_issue");
			if (readTool?.renderResult === undefined) throw new Error("missing renderer");
			const final = readResults.at(-1);
			if (final?.type !== "message" || final.message.role !== "toolResult") throw new Error("missing final read");
			const displayed: string[] = [];
			readTool.renderResult(
				{ content: final.message.content, details: final.message.details },
				{ expanded: true, isPartial: false },
				{
					fg(color: string, value: string) {
						if (color === "toolOutput") displayed.push(value);
						return value;
					},
					bold: (value: string) => value,
				} as never,
				{ args: { number: 41 } } as never,
			);
			expect(displayed.join("\n")).toContain("# Parser <tags>");
			expect(displayed.join("\n")).toContain("A&B comment");
		} finally {
			session.dispose();
		}
	});
});
