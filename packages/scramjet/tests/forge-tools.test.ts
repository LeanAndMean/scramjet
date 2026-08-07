import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
	AuthStorage,
	createAgentSession,
	type ExecResult,
	type ExtensionAPI,
	ModelRegistry,
	SessionManager,
	SettingsManager,
	type ToolDefinition,
} from "@leanandmean/coding-agent";
import { Value } from "typebox/value";
import { describe, expect, it, vi } from "vitest";
import { ForgeCommandError, runForgeCommand, UnsupportedForgeOriginError } from "../src/forge/client.js";
import { renderForgeDocument, sliceForgeDocument } from "../src/forge/document.js";
import { registerForgeTools } from "../src/forge/tools.js";
import type { ForgeAdapter, ForgeArtifact, ForgeIssue, ForgePullRequest, ForgeRepository } from "../src/forge/types.js";
import { recordingPi } from "./helpers.js";

const githubRepository: ForgeRepository = {
	forge: "github",
	host: "github.com",
	projectPath: "Acme/widget",
};

const gitlabRepository: ForgeRepository = {
	forge: "gitlab",
	host: "gitlab.com",
	projectPath: "Acme/platform/widget",
};

function issue(overrides: Partial<ForgeIssue> = {}): ForgeIssue {
	return {
		kind: "issue",
		number: 7,
		url: "https://github.com/Acme/widget/issues/7",
		state: "open",
		author: { login: "alice", kind: "user" },
		createdAt: "2026-01-01T00:00:00Z",
		updatedAt: "2026-01-02T00:00:00Z",
		labels: ["bug"],
		assignees: [{ login: "bob", kind: "user" }],
		title: "Parser failure",
		body: "first line\nsecond line",
		comments: [
			{
				id: "101",
				url: "https://github.com/Acme/widget/issues/7#issuecomment-101",
				author: { login: "helper[bot]", kind: "bot" },
				body: "Investigating",
				createdAt: "2026-01-03T00:00:00Z",
				updatedAt: "2026-01-03T00:00:00Z",
			},
		],
		relationships: { capability: "supported", items: [] },
		...overrides,
	};
}

function pullRequest(overrides: Partial<ForgePullRequest> = {}): ForgePullRequest {
	return {
		kind: "pr",
		number: 12,
		url: "https://github.com/Acme/widget/pull/12",
		state: "open",
		author: { login: "alice", kind: "user" },
		createdAt: "2026-01-01T00:00:00Z",
		updatedAt: "2026-01-02T00:00:00Z",
		labels: [],
		assignees: [],
		title: "Ship parser fix",
		body: "Ready",
		comments: [],
		readiness: {
			draft: false,
			mergeable: "mergeable",
			reviewDecision: { capability: "supported", value: "approved" },
			head: "feature/parser",
			base: "main",
		},
		sections: {},
		...overrides,
	};
}

function execResult(overrides: Partial<ExecResult> = {}): ExecResult {
	return { stdout: "", stderr: "", code: 0, killed: false, ...overrides };
}

function adapterFor(readArtifact: ForgeAdapter["readArtifact"], overrides: Partial<ForgeAdapter> = {}): ForgeAdapter {
	return {
		readArtifact,
		createArtifact: vi.fn(),
		updateArtifact: vi.fn(),
		addComment: vi.fn(),
		updateComment: vi.fn(),
		...overrides,
	};
}

function toolSetup(
	artifact: ForgeArtifact | (() => ForgeArtifact),
	overrides: Partial<ForgeAdapter> = {},
	repository = githubRepository,
) {
	const bag = recordingPi();
	bag.pi.exec = vi.fn(async (command: string, args: string[]) => {
		if (command === "git" && args.join(" ") === "remote get-url origin") {
			return execResult({ stdout: `https://${repository.host}/${repository.projectPath}.git\n` });
		}
		throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
	});
	const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () =>
		typeof artifact === "function" ? artifact() : artifact,
	);
	const adapter = adapterFor(readArtifact, overrides);
	registerForgeTools(bag.pi, { createAdapter: () => adapter });
	return {
		bag,
		adapter,
		readArtifact,
		issueTool: bag.tools.find((tool) => tool.name === "read_issue"),
		prTool: bag.tools.find((tool) => tool.name === "read_pr"),
		createIssueTool: bag.tools.find((tool) => tool.name === "create_issue"),
		createPrTool: bag.tools.find((tool) => tool.name === "create_pr"),
		addIssueCommentTool: bag.tools.find((tool) => tool.name === "add_issue_comment"),
		addPrCommentTool: bag.tools.find((tool) => tool.name === "add_pr_comment"),
		editIssueTool: bag.tools.find((tool) => tool.name === "edit_issue"),
		editPrTool: bag.tools.find((tool) => tool.name === "edit_pr"),
	};
}

function toolContext(entries: unknown[] = []) {
	return { cwd: "/repo", sessionManager: { getBranch: () => entries } };
}

let branchEntryId = 0;

function branchEntry(type: string, value: Record<string, unknown>) {
	return {
		type,
		id: `entry-${++branchEntryId}`,
		parentId: null,
		timestamp: "2026-01-01T00:00:00Z",
		...value,
	};
}

function assistantEntry(calls: Array<{ id: string; name: string }>) {
	return branchEntry("message", {
		message: {
			role: "assistant",
			content: calls.map((call) => ({ type: "toolCall", id: call.id, name: call.name, arguments: {} })),
		},
	});
}

function readResultEntry(toolCallId: string, toolName: "read_issue" | "read_pr", details: unknown, isError = false) {
	return branchEntry("message", {
		message: {
			role: "toolResult",
			toolCallId,
			toolName,
			content: [{ type: "text", text: "persisted read" }],
			details,
			isError,
		},
	});
}

function evidenceBranch(
	details: unknown[],
	readTool: "read_issue" | "read_pr",
	currentCalls: Array<{ id: string; name: string }>,
) {
	return [
		...details.flatMap((receipt, index) => [
			assistantEntry([{ id: `read-${index}`, name: readTool }]),
			readResultEntry(`read-${index}`, readTool, receipt),
		]),
		assistantEntry(currentCalls),
	];
}

function fullReceipt(artifact: ForgeArtifact, repository = githubRepository) {
	return sliceForgeDocument(renderForgeDocument(repository, artifact), {}).details;
}

function completeReceipts(artifact: ForgeArtifact) {
	const rendered = renderForgeDocument(githubRepository, artifact);
	const receipts = [];
	let offset = 1;
	while (true) {
		const slice = sliceForgeDocument(rendered, {
			offset,
			...(offset === 1 ? {} : { snapshot: rendered.snapshot }),
		});
		receipts.push(slice.details);
		if (slice.nextOffset === undefined) return receipts;
		offset = slice.nextOffset;
	}
}

function fieldReceipts(
	artifact: ForgeArtifact,
	target: { kind: "artifact" } | { kind: "comment"; id: string },
	field: "title" | "body",
	repository = githubRepository,
) {
	const rendered = renderForgeDocument(repository, artifact);
	const lines = [
		...new Set(
			rendered.fieldSpans
				.filter(
					(span) =>
						span.field === field &&
						span.target.kind === target.kind &&
						(span.target.kind === "artifact" || span.target.id === (target as { id: string }).id),
				)
				.map((span) => span.line),
		),
	];
	return lines.map(
		(line) =>
			sliceForgeDocument(rendered, {
				offset: line + 1,
				limit: 1,
				snapshot: rendered.snapshot,
			}).details,
	);
}

function theme() {
	return {
		fg: (_color: string, value: string) => value,
		bold: (value: string) => value,
	};
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

async function expectForgeFailure(promise: Promise<any>, failureClass: string, message?: RegExp) {
	const result = await promise;
	expect(result).toMatchObject({
		isError: true,
		details: { schema: "scramjet:forge-failure@1", class: failureClass },
	});
	if (message) expect(result.content[0].text).toMatch(message);
	expect(Buffer.byteLength(result.content[0].text, "utf8")).toBeLessThanOrEqual(1024);
	return result;
}

describe("registerForgeTools read contracts", () => {
	it("registers independently named, strict model-callable forge tools", () => {
		const {
			bag,
			issueTool,
			prTool,
			createIssueTool,
			createPrTool,
			addIssueCommentTool,
			addPrCommentTool,
			editIssueTool,
			editPrTool,
		} = toolSetup(issue());
		expect(bag.tools.map((tool) => tool.name)).toEqual([
			"read_issue",
			"read_pr",
			"create_issue",
			"create_pr",
			"add_issue_comment",
			"add_pr_comment",
			"edit_issue",
			"edit_pr",
		]);
		const providerMetadata = JSON.stringify(
			bag.tools.map(({ name, description, parameters }) => ({ name, description, parameters })),
		);
		const promptMetadata = JSON.stringify(
			bag.tools.map(({ promptSnippet, promptGuidelines }) => ({ promptSnippet, promptGuidelines })),
		);
		expect(Buffer.byteLength(providerMetadata + promptMetadata, "utf8")).toBeLessThanOrEqual(6400);
		for (const tool of [issueTool, prTool]) {
			expect(tool.activation).toBeUndefined();
			expect(tool.description).toContain("^!HHHH; decodes once");
			expect(tool.promptSnippet).toEqual(expect.any(String));
			expect(tool.promptGuidelines).toEqual(expect.arrayContaining([expect.stringContaining(tool.name)]));
			expect(tool.parameters.additionalProperties).toBe(false);
		}

		expect(Value.Check(issueTool.parameters, { number: 7 })).toBe(true);
		expect(Value.Check(issueTool.parameters, { number: 7, offset: 2, limit: 4, snapshot: "a".repeat(64) })).toBe(
			true,
		);
		expect(Value.Check(issueTool.parameters, { number: 0 })).toBe(false);
		expect(Value.Check(issueTool.parameters, { number: 7, include: ["files"] })).toBe(false);
		expect(Value.Check(issueTool.parameters, { number: 7, extra: true })).toBe(false);

		expect(Value.Check(prTool.parameters, { number: 12, include: ["files", "checks"] })).toBe(true);
		expect(Value.Check(prTool.parameters, { number: 12, include: ["files", "files"] })).toBe(false);
		expect(Value.Check(prTool.parameters, { number: 12, include: ["reviews"] })).toBe(false);
		expect(Value.Check(prTool.parameters, { number: 12, snapshot: `${"ABC".repeat(21)}A` })).toBe(false);

		for (const tool of [createIssueTool, createPrTool, addIssueCommentTool, addPrCommentTool]) {
			expect(tool.activation).toBeUndefined();
			expect(tool.promptSnippet).toEqual(expect.any(String));
			expect(tool.parameters.additionalProperties).toBe(false);
		}
		expect(Value.Check(createIssueTool.parameters, { title: "Issue", body: "" })).toBe(true);
		expect(Value.Check(createIssueTool.parameters, { title: "", body: "Body" })).toBe(false);
		expect(Value.Check(createPrTool.parameters, { title: "PR", body: "Body", head: "feature", base: "main" })).toBe(
			true,
		);
		expect(
			Value.Check(createPrTool.parameters, {
				title: "PR",
				body: "Body",
				head: "feature",
				base: "main",
				draft: true,
			}),
		).toBe(true);
		expect(Value.Check(createPrTool.parameters, { title: "PR", body: "Body", head: "feature" })).toBe(false);
		expect(Value.Check(addIssueCommentTool.parameters, { number: 7, body: "Comment" })).toBe(true);
		expect(Value.Check(addPrCommentTool.parameters, { number: 12, body: "", extra: true })).toBe(false);

		for (const tool of [editIssueTool, editPrTool]) {
			expect(tool.activation).toBeUndefined();
			expect(tool.description).toContain("^!HHHH; read escapes decode once");
			expect(tool.promptSnippet).toEqual(expect.any(String));
			expect(tool.parameters.type).toBe("object");
			expect(Object.keys(tool.parameters.properties)).toEqual(["number", "target", "edits"]);
			expect(
				Value.Check(tool.parameters, {
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "old", newText: "new" }],
				}),
			).toBe(true);
			expect(
				Value.Check(tool.parameters, {
					number: 7,
					target: { kind: "comment", id: "opaque" },
					edits: [{ field: "body", oldText: "old", newText: "new" }],
				}),
			).toBe(true);
			expect(
				Value.Check(tool.parameters, {
					number: 7,
					target: { kind: "artifact", id: "other-object" },
					edits: [{ field: "title", oldText: "old", newText: "new" }],
				}),
			).toBe(false);
			expect(
				Value.Check(tool.parameters, {
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "state", oldText: "open", newText: "closed" }],
				}),
			).toBe(false);
			expect(tool.promptGuidelines?.join(" ")).toContain("target one object");
		}
	});

	it("fetches and validates the aggregate issue before returning the canonical bracket document", async () => {
		const { issueTool, readArtifact } = toolSetup(issue());
		const controller = new AbortController();
		const result = await issueTool.execute("read-1", { number: 7 }, controller.signal, undefined, toolContext());

		expect(readArtifact).toHaveBeenCalledWith(githubRepository, "issue", 7, [], controller.signal);
		expect(result.content[0].text).toContain('^artifact format="forge-caret-1" content-trust="untrusted"');
		expect(result.content[0].text).toContain('^comment id="101"');
		expect(result.details).toMatchObject({
			schema: "scramjet:forge-read@1",
			repository: githubRepository,
			artifact: { kind: "issue", number: 7 },
			range: { offset: 1 },
		});
		expect(result.details.fields).toEqual(
			expect.arrayContaining([
				expect.objectContaining({ target: { kind: "artifact" }, field: "title", ranges: [{ start: 0, end: 14 }] }),
				expect.objectContaining({ target: { kind: "comment", id: "101" }, field: "body" }),
			]),
		);
		expect(result.details.core.ranges).toEqual([{ start: 0, end: result.details.core.totalLines }]);
	});

	it("passes opt-in PR sections and keeps comments before fixed-order bulky sections", async () => {
		const artifact = pullRequest({
			comments: [
				{
					id: "20",
					url: "https://github.com/Acme/widget/pull/12#issuecomment-20",
					author: { login: "alice", kind: "user" },
					body: "conversation",
					createdAt: "2026-01-03T00:00:00Z",
					updatedAt: "2026-01-03T00:00:00Z",
				},
			],
			sections: {
				checks: [{ id: "1", name: "test", status: "completed", conclusion: "success", url: null }],
				files: [{ path: "src/a.ts", status: "modified", additions: 2, deletions: 1, previousPath: null }],
			},
		});
		const { prTool, readArtifact } = toolSetup(artifact);
		const result = await prTool.execute(
			"read-pr-1",
			{ number: 12, include: ["checks", "files"] },
			undefined,
			undefined,
			toolContext(),
		);
		const text = result.content[0].text as string;

		expect(readArtifact).toHaveBeenCalledWith(githubRepository, "pr", 12, ["checks", "files"], undefined);
		expect(text.indexOf("^comments{")).toBeLessThan(text.indexOf("^files{"));
		expect(text.indexOf("^files{")).toBeLessThan(text.indexOf("^checks{"));
		expect(result.details.include).toEqual(["files", "checks"]);
	});

	it("keeps opt-in PR sections in lossless continuation guidance", async () => {
		const bag = recordingPi();
		bag.pi.exec = vi.fn(async () => execResult({ stdout: "https://github.com/Acme/widget.git\n" }));
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async (_repository, _kind, _number, include) =>
			pullRequest({
				sections: include.includes("files")
					? { files: [{ path: "src/a.ts", status: "modified", additions: 1, deletions: 0, previousPath: null }] }
					: {},
			}),
		);
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(readArtifact) });
		const tool = bag.tools.find((candidate) => candidate.name === "read_pr");

		const first = await tool.execute(
			"pr-range-1",
			{ number: 12, include: ["files"], limit: 1 },
			undefined,
			undefined,
			toolContext(),
		);
		expect(first.content[0].text).toContain("^continue ");
		expect(first.content[0].text).toContain('include="files"');

		await expect(
			tool.execute(
				"pr-range-2",
				{ number: 12, include: ["files"], offset: 2, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
		).resolves.toMatchObject({ details: { snapshot: first.details.snapshot } });
		await expectForgeFailure(
			tool.execute(
				"pr-range-3",
				{ number: 12, offset: 2, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_READ_FAILED",
			/snapshot changed/i,
		);
	});

	it("returns exact range receipts and requires a stable snapshot for continuation", async () => {
		let current = issue({ body: `${"line\n".repeat(100)}end` });
		const { issueTool, readArtifact } = toolSetup(() => current);
		const rendered = renderForgeDocument(githubRepository, current);
		const bodyLine = rendered.fieldSpans.find(
			(span) => span.target.kind === "artifact" && span.field === "body",
		)?.line;
		if (bodyLine === undefined) throw new Error("missing body span");

		const first = await issueTool.execute(
			"range-1",
			{ number: 7, offset: bodyLine + 1, limit: 2 },
			undefined,
			undefined,
			toolContext(),
		);
		expect(first.details.range).toEqual({ offset: bodyLine + 1, lines: 2, totalLines: rendered.lines.length });
		expect(first.details.fields).toContainEqual({
			target: { kind: "artifact" },
			field: "body",
			totalCodeUnits: current.body.length,
			ranges: [{ start: 0, end: "line\nline\n".length }],
		});
		expect(first.content[0].text).toContain(`snapshot="${first.details.snapshot}"`);

		const continuation = await issueTool.execute(
			"range-2",
			{ number: 7, offset: bodyLine + 3, limit: 2, snapshot: first.details.snapshot },
			undefined,
			undefined,
			toolContext(),
		);
		expect(continuation.details.snapshot).toBe(first.details.snapshot);
		expect(readArtifact).toHaveBeenCalledTimes(2);

		current = issue({ ...current, title: "Changed externally" });
		await expectForgeFailure(
			issueTool.execute(
				"range-3",
				{ number: 7, offset: bodyLine + 5, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_READ_FAILED",
			/snapshot changed/i,
		);
		expect(readArtifact).toHaveBeenCalledTimes(3);
	});

	it("enforces a read-only forge tool set through the real AgentSession pipeline", async () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-forge-tool-"));
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		mkdirSync(cwd);
		const tools: ToolDefinition[] = [];
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => issue());
		const adapter = adapterFor(readArtifact);
		const pi = {
			registerTool(tool: ToolDefinition) {
				tools.push(tool);
			},
			on() {},
			exec: async () => execResult({ stdout: "https://github.com/Acme/widget.git\n" }),
		} as unknown as ExtensionAPI;
		registerForgeTools(pi, { createAdapter: () => adapter });
		const sessionManager = SessionManager.inMemory(cwd);
		const authStorage = AuthStorage.inMemory();
		let session: Awaited<ReturnType<typeof createAgentSession>>["session"] | undefined;
		try {
			({ session } = await createAgentSession({
				cwd,
				agentDir,
				authStorage,
				modelRegistry: ModelRegistry.inMemory(authStorage),
				sessionManager,
				settingsManager: SettingsManager.inMemory(),
				customTools: tools,
			}));
			expect(session.getActiveToolNames()).toContain("bash");
			session.setActiveToolsByName(["read_issue", "read_pr"]);
			expect(session.getActiveToolNames()).toEqual(["read_issue", "read_pr"]);
			for (const unavailable of [
				"create_issue",
				"create_pr",
				"add_issue_comment",
				"add_pr_comment",
				"edit_issue",
				"edit_pr",
				"bash",
			]) {
				expect(session.getActiveToolNames()).not.toContain(unavailable);
			}
			await session.invokeHarnessTool("read_issue", { number: 7 });

			const result = sessionManager
				.getBranch()
				.findLast(
					(entry) =>
						entry.type === "message" &&
						entry.message.role === "toolResult" &&
						entry.message.toolName === "read_issue",
				);
			expect(result).toBeDefined();
			if (result?.type !== "message" || result.message.role !== "toolResult") throw new Error("missing tool result");
			expect(result.message.isError).toBe(false);
			expect(result.message.details).toMatchObject({
				schema: "scramjet:forge-read@1",
				repository: githubRepository,
				artifact: { kind: "issue", number: 7 },
				range: { offset: 1 },
			});

			readArtifact.mockRejectedValueOnce(
				new ForgeCommandError("failed", {
					command: "gh",
					args: ["api", "repos/Acme/widget/issues/7"],
					cwd,
					process: { exitCode: 1, stdout: "", stderr: "HTTP 403 forbidden" },
				}),
			);
			await session.invokeHarnessTool("read_issue", { number: 7 });
			const failure = sessionManager
				.getBranch()
				.findLast(
					(entry) =>
						entry.type === "message" &&
						entry.message.role === "toolResult" &&
						entry.message.toolName === "read_issue",
				);
			if (failure?.type !== "message" || failure.message.role !== "toolResult") {
				throw new Error("missing forge failure result");
			}
			expect(failure.message).toMatchObject({
				isError: true,
				details: {
					schema: "scramjet:forge-failure@1",
					class: "FORGE_READ_FAILED",
					operation: "read_issue",
					writeState: "not_attempted",
				},
			});
			expect(failure.message.content[0]).toMatchObject({
				type: "text",
				text: expect.stringContaining("read-only gh"),
			});
		} finally {
			session?.dispose();
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("retains diagnostic evidence when an unexpected message exceeds the model budget", async () => {
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => {
			throw new TypeError("x".repeat(2000));
		});
		const { issueTool } = toolSetup(issue(), { readArtifact });
		const result = await expectForgeFailure(
			issueTool.execute("unexpected", { number: 7 }, undefined, undefined, toolContext()),
			"FORGE_READ_FAILED",
			/truncated 2000 bytes\/1 lines; sha256/i,
		);
		expect(result.details.trace).toContain("forge-tools.test.ts");

		const multiline = vi.fn<ForgeAdapter["readArtifact"]>(async () => {
			throw new TypeError(Array.from({ length: 100 }, (_, index) => `line ${index}`).join("\n"));
		});
		const multilineSetup = toolSetup(issue(), { readArtifact: multiline });
		await expectForgeFailure(
			multilineSetup.issueTool.execute("multiline", { number: 7 }, undefined, undefined, toolContext()),
			"FORGE_READ_FAILED",
			/truncated \d+ bytes\/100 lines; sha256/i,
		);
	});

	it("classifies an unsupported origin as repository failure without selecting a CLI fallback", async () => {
		const bag = recordingPi();
		registerForgeTools(bag.pi, {
			resolveRepository: async () => {
				throw new UnsupportedForgeOriginError();
			},
		});
		const tool = bag.tools.find((candidate) => candidate.name === "read_issue");
		const result = await expectForgeFailure(
			tool.execute("unsupported", { number: 7 }, undefined, undefined, toolContext()),
			"FORGE_READ_FAILED",
			/supported public GitHub or GitLab/i,
		);
		expect(result.details.phase).toBe("repository");
		expect(result.content[0].text).not.toContain("read-only gh");
		expect(result.content[0].text).not.toContain("read-only glab");
	});

	it("renders compactly from persisted data without performing remote work", async () => {
		const { issueTool, readArtifact } = toolSetup(issue());
		const result = await issueTool.execute("render-1", { number: 7 }, undefined, undefined, toolContext());
		readArtifact.mockClear();

		const call = issueTool.renderCall({ number: 7, offset: 2, limit: 5 }, theme(), { lastComponent: undefined });
		expect(call.render(120).join("\n")).toContain("read_issue #7");

		const collapsed = issueTool.renderResult(result, { expanded: false, isPartial: false }, theme(), {
			args: { number: 7 },
			lastComponent: undefined,
		});
		expect(collapsed.render(120).join("\n")).toMatch(/positions 1-\d+ of \d+/);
		const expanded = issueTool.renderResult(result, { expanded: true, isPartial: false }, theme(), {
			args: { number: 7 },
			lastComponent: undefined,
		});
		const expandedText = expanded.render(10_000).join("\n").replace(/ +$/gm, "");
		const { offset, lines, totalLines } = result.details.range;
		expect(expandedText).toBe(
			`positions ${offset}-${offset + lines - 1} of ${totalLines}\n${result.content[0].text}`,
		);
		expect(expandedText).toContain("^body{first line");
		const displayedPayloads: string[] = [];
		const spyTheme = {
			fg: (color: string, value: string) => {
				if (color === "toolOutput") displayedPayloads.push(value);
				return value;
			},
			bold: (value: string) => value,
		};
		issueTool.renderResult(result, { expanded: true, isPartial: false }, spyTheme, {
			args: { number: 7 },
			lastComponent: undefined,
		});
		expect(displayedPayloads).toEqual([result.content[0].text]);

		const replayFallback = issueTool.renderResult(
			{ content: [{ type: "text", text: "persisted text" }], details: { malformed: true } },
			{ expanded: true, isPartial: false },
			theme(),
			{ args: { number: 7 }, lastComponent: undefined },
		);
		expect(replayFallback.render(120).join("\n")).toContain("persisted text");
		expect(readArtifact).not.toHaveBeenCalled();
	});
});

describe("registerForgeTools creation contracts", () => {
	function addedComment(id: string, body: string) {
		return {
			id,
			url: `https://github.com/Acme/widget/issues/7#issuecomment-${id}`,
			author: { login: "alice", kind: "user" as const },
			body,
			createdAt: "2026-01-04T00:00:00Z",
			updatedAt: "2026-01-04T00:00:00Z",
		};
	}

	it("creates artifacts without read evidence, refetches the returned identity, and verifies explicit PR inputs", async () => {
		const createdIssue = issue({
			number: 41,
			url: "https://github.com/Acme/widget/issues/41",
			title: "Created issue",
			body: "Issue body",
			comments: [],
		});
		const createdPr = pullRequest({
			number: 42,
			url: "https://github.com/Acme/widget/pull/42",
			title: "Created PR",
			body: "PR body",
			readiness: { ...pullRequest().readiness, draft: true, head: "feature", base: "main" },
		});
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async (_repository, input) =>
			input.kind === "issue"
				? { kind: "issue", number: 41, url: createdIssue.url }
				: { kind: "pr", number: 42, url: createdPr.url },
		);
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async (_repository, kind, number) => {
			if (kind === "issue" && number === 41) return createdIssue;
			if (kind === "pr" && number === 42) return createdPr;
			throw new Error(`Unexpected refetch ${kind} #${number}`);
		});
		const { createIssueTool, createPrTool } = toolSetup(issue(), { createArtifact, readArtifact });

		const issueResult = await createIssueTool.execute(
			"create-issue",
			{ title: "Created issue", body: "Issue body" },
			undefined,
			undefined,
			toolContext(),
		);
		const prResult = await createPrTool.execute(
			"create-pr",
			{ title: "Created PR", body: "PR body", head: "feature", base: "main", draft: true },
			undefined,
			undefined,
			toolContext(),
		);

		expect(createArtifact).toHaveBeenNthCalledWith(
			1,
			githubRepository,
			{ kind: "issue", title: "Created issue", body: "Issue body" },
			undefined,
		);
		expect(createArtifact).toHaveBeenNthCalledWith(
			2,
			githubRepository,
			{ kind: "pr", title: "Created PR", body: "PR body", head: "feature", base: "main", draft: true },
			undefined,
		);
		expect(readArtifact.mock.calls.map((call) => call.slice(1, 4))).toEqual([
			["issue", 41, []],
			["pr", 42, []],
		]);
		expect(issueResult.content[0].text).toBe(createdIssue.url);
		expect(prResult.content[0].text).toBe(createdPr.url);
		expect(Buffer.byteLength(issueResult.content[0].text, "utf8")).toBeLessThanOrEqual(512);
		expect(JSON.stringify(issueResult)).not.toContain("Issue body");
		expect(JSON.stringify(prResult)).not.toContain("PR body");
		expect(issueResult.details).toMatchObject({
			schema: "scramjet:forge-mutation@1",
			operation: "create_issue",
			identity: { kind: "issue", number: 41, url: createdIssue.url },
			verified: true,
		});
	});

	it("authorizes comment creation from complete same-snapshot evidence and keeps its receipt visible", async () => {
		const original = issue({ body: "x".repeat(60_000) });
		const receipts = completeReceipts(original);
		expect(receipts.length).toBeGreaterThan(1);
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const addComment = vi.fn<ForgeAdapter["addComment"]>(async () => {
			const comment = addedComment("202", "New comment");
			current = issue({ ...current, comments: [...current.comments, comment] });
			return { kind: "comment", id: comment.id, url: comment.url };
		});
		const { addIssueCommentTool } = toolSetup(original, { readArtifact, addComment });
		const branch = evidenceBranch(receipts, "read_issue", [{ id: "add-comment", name: "add_issue_comment" }]);

		const result = await addIssueCommentTool.execute(
			"add-comment",
			{ number: 7, body: "New comment" },
			undefined,
			undefined,
			toolContext(branch),
		);

		expect(readArtifact).toHaveBeenCalledTimes(2);
		expect(addComment).toHaveBeenCalledWith(
			githubRepository,
			{ kind: "issue", number: 7, body: "New comment" },
			undefined,
		);
		expect(result.content[0].text).toBe(current.comments.at(-1)?.url);
		expect(JSON.stringify(result)).not.toContain("New comment");
		expect(Buffer.byteLength(result.content[0].text, "utf8")).toBeLessThanOrEqual(512);
		expect(result.details).toMatchObject({
			operation: "add_issue_comment",
			identity: { kind: "comment", id: "202" },
			verified: true,
		});
	});

	it.each([
		{
			name: "has no prior read",
			branch: (_receipt: unknown) => evidenceBranch([], "read_issue", [{ id: "add", name: "add_issue_comment" }]),
		},
		{
			name: "uses a read from the same assistant batch",
			branch: (receipt: unknown) => [
				assistantEntry([
					{ id: "same-batch-read", name: "read_issue" },
					{ id: "add", name: "add_issue_comment" },
				]),
				readResultEntry("same-batch-read", "read_issue", receipt),
			],
		},
		{
			name: "uses evidence before compaction",
			branch: (receipt: unknown) => [
				assistantEntry([{ id: "old-read", name: "read_issue" }]),
				readResultEntry("old-read", "read_issue", receipt),
				branchEntry("compaction", { summary: "compact", firstKeptEntryId: "entry-1", tokensBefore: 1 }),
				assistantEntry([{ id: "add", name: "add_issue_comment" }]),
			],
		},
		{
			name: "uses evidence for another repository",
			branch: (receipt: any) =>
				evidenceBranch(
					[{ ...receipt, repository: { ...receipt.repository, projectPath: "Other/widget" } }],
					"read_issue",
					[{ id: "add", name: "add_issue_comment" }],
				),
		},
	])("rejects comment creation that $name", async ({ branch }) => {
		const original = issue();
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
		const addComment = vi.fn<ForgeAdapter["addComment"]>();
		const { addIssueCommentTool } = toolSetup(original, { readArtifact, addComment });

		await expectForgeFailure(
			addIssueCommentTool.execute(
				"add",
				{ number: 7, body: "New comment" },
				undefined,
				undefined,
				toolContext(branch(fullReceipt(original))),
			),
			"FORGE_PREFLIGHT_FAILED",
			/complete prior read_issue/i,
		);
		expect(readArtifact).not.toHaveBeenCalled();
		expect(addComment).not.toHaveBeenCalled();
	});

	it("rejects incomplete and mixed-snapshot parent evidence before comment creation", async () => {
		const original = issue({ body: `${"line\n".repeat(3000)}end` });
		const receipts = completeReceipts(original);
		expect(receipts.length).toBeGreaterThan(1);
		const branches = [
			evidenceBranch([receipts[0]], "read_issue", [{ id: "add", name: "add_issue_comment" }]),
			evidenceBranch(
				receipts.map((receipt, index) => ({
					...receipt,
					snapshot: index === 0 ? "a".repeat(64) : "b".repeat(64),
				})),
				"read_issue",
				[{ id: "add", name: "add_issue_comment" }],
			),
		];
		for (const branch of branches) {
			const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
			const addComment = vi.fn<ForgeAdapter["addComment"]>();
			const { addIssueCommentTool } = toolSetup(original, { readArtifact, addComment });
			await expectForgeFailure(
				addIssueCommentTool.execute(
					"add",
					{ number: 7, body: "New comment" },
					undefined,
					undefined,
					toolContext(branch),
				),
				"FORGE_PREFLIGHT_FAILED",
				/complete prior read_issue/i,
			);
			expect(readArtifact).not.toHaveBeenCalled();
			expect(addComment).not.toHaveBeenCalled();
		}
	});

	it("rejects deterministic GitLab draft-title conflicts before attempting creation", async () => {
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>();
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>();
		const { createPrTool } = toolSetup(pullRequest(), { createArtifact, readArtifact }, gitlabRepository);

		await expectForgeFailure(
			createPrTool.execute(
				"create-pr",
				{ title: "Draft: Release", body: "Body", head: "feature", base: "main" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_PREFLIGHT_FAILED",
			/cannot create a non-draft merge request/i,
		);
		expect(createArtifact).not.toHaveBeenCalled();
		expect(readArtifact).not.toHaveBeenCalled();
	});

	it("rejects an unprefixed GitLab draft title before attempting creation", async () => {
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>();
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>();
		const { createPrTool } = toolSetup(pullRequest(), { createArtifact, readArtifact }, gitlabRepository);

		await expectForgeFailure(
			createPrTool.execute(
				"create-pr",
				{ title: "Release", body: "Body", head: "feature", base: "main", draft: true },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_PREFLIGHT_FAILED",
			/exact approved title.*draft prefix/i,
		);
		expect(createArtifact).not.toHaveBeenCalled();
		expect(readArtifact).not.toHaveBeenCalled();
	});

	it("treats generic post-send CLI failures as ambiguous for artifact and comment creation", async () => {
		const failure = new ForgeCommandError("failed", { command: "gh", args: ["api"], cwd: "/repo" });
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => {
			throw failure;
		});
		const createSetup = toolSetup(issue(), { createArtifact });
		await expectForgeFailure(
			createSetup.createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);

		const original = issue();
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
		const addComment = vi.fn<ForgeAdapter["addComment"]>(async () => {
			throw failure;
		});
		const commentSetup = toolSetup(original, { readArtifact, addComment });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [{ id: "add", name: "add_issue_comment" }]);
		await expectForgeFailure(
			commentSetup.addIssueCommentTool.execute(
				"add",
				{ number: 7, body: "Comment" },
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(addComment).toHaveBeenCalledTimes(1);
		expect(readArtifact).toHaveBeenCalledTimes(1);
	});

	it.each(["timeout", "cancelled", "stdin"] as const)("treats a post-send %s failure as ambiguous", async (kind) => {
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => {
			throw new ForgeCommandError(kind, { command: "gh", args: ["api"], cwd: "/repo" });
		});
		const { createIssueTool } = toolSetup(issue(), { createArtifact });
		await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(createArtifact).toHaveBeenCalledTimes(1);
	});

	it("treats a partial mutation-content authentication echo as ambiguous", async () => {
		const stdin = JSON.stringify({ body: "heading\nHTTP 401 Unauthorized\nfooter" });
		let failure: unknown;
		try {
			await runForgeCommand(async () => execResult({ code: 1, stderr: "HTTP 401 Unauthorized" }), {
				command: "gh",
				args: ["api"],
				cwd: "/repo",
				stdin,
			});
		} catch (error) {
			failure = error;
		}
		expect(failure).toMatchObject({ invocation: { process: { authenticationFailure: false } } });
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => {
			throw failure;
		});
		const { createIssueTool } = toolSetup(issue(), { createArtifact });
		const result = await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(result.content[0].text).not.toMatch(/then retry|retry after|retry the/i);
	});

	it("reports only a missing executable as a conclusive write rejection", async () => {
		const failure = new ForgeCommandError("missing-executable", { command: "gh", args: ["api"], cwd: "/repo" });
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => {
			throw failure;
		});
		const { createIssueTool } = toolSetup(issue(), { createArtifact });
		await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_REJECTED",
			/no request reached/i,
		);
	});

	it("uses authentication diagnostics as guidance without proving mutation rejection", async () => {
		const failure = new ForgeCommandError("failed", {
			command: "gh",
			args: ["api"],
			cwd: "/repo",
			process: { exitCode: 1, stdout: "", stderr: "HTTP 401 Unauthorized" },
		});
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => {
			throw failure;
		});
		const { createIssueTool } = toolSetup(issue(), { createArtifact });
		await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
	});

	it("treats a failed post-identity verification read as ambiguous", async () => {
		const verificationFailure = new ForgeCommandError("failed", { command: "gh", args: ["api"], cwd: "/repo" });
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => ({
			kind: "issue",
			number: 41,
			url: "https://github.com/Acme/widget/issues/41",
		}));
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => {
			throw verificationFailure;
		});
		const { createIssueTool } = toolSetup(issue(), { createArtifact, readArtifact });

		await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Issue body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(createArtifact).toHaveBeenCalledTimes(1);
		expect(readArtifact).toHaveBeenCalledTimes(1);
	});

	it("reports a possibly successful creation when exact post-write verification fails without retrying", async () => {
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => ({
			kind: "issue",
			number: 41,
			url: "https://github.com/Acme/widget/issues/41",
		}));
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () =>
			issue({
				number: 41,
				url: "https://github.com/Acme/widget/issues/41",
				title: "Changed during creation",
				body: "Issue body",
			}),
		);
		const { createIssueTool } = toolSetup(issue(), { createArtifact, readArtifact });

		const failure = await expectForgeFailure(
			createIssueTool.execute(
				"create",
				{ title: "Created issue", body: "Issue body" },
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(failure.content[0].text).toMatch(/created artifact content did not match/i);
		expect(failure.details.trace).toContain("verifyCreatedArtifact");
		expect(createArtifact).toHaveBeenCalledTimes(1);
		expect(readArtifact).toHaveBeenCalledTimes(1);
	});

	it("serializes comment creation by parent artifact through verification", async () => {
		const original = issue();
		let current = original;
		let releaseFirst!: () => void;
		const firstPending = new Promise<void>((resolve) => {
			releaseFirst = resolve;
		});
		let mutation = 0;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const addComment = vi.fn<ForgeAdapter["addComment"]>(async (_repository, input) => {
			mutation++;
			if (mutation === 1) await firstPending;
			const id = String(201 + mutation);
			const comment = addedComment(id, input.body);
			current = issue({ ...current, comments: [...current.comments, comment] });
			return { kind: "comment", id, url: comment.url };
		});
		const { addIssueCommentTool } = toolSetup(original, { readArtifact, addComment });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [
			{ id: "add-first", name: "add_issue_comment" },
			{ id: "add-second", name: "add_issue_comment" },
		]);

		const first = addIssueCommentTool.execute(
			"add-first",
			{ number: 7, body: "First" },
			undefined,
			undefined,
			toolContext(branch),
		);
		await vi.waitFor(() => expect(addComment).toHaveBeenCalledTimes(1));
		const second = addIssueCommentTool.execute(
			"add-second",
			{ number: 7, body: "Second" },
			undefined,
			undefined,
			toolContext(branch),
		);
		await flush();
		expect(readArtifact).toHaveBeenCalledTimes(1);
		expect(addComment).toHaveBeenCalledTimes(1);

		releaseFirst();
		await expect(Promise.all([first, second])).resolves.toHaveLength(2);
		expect(addComment.mock.calls.map((call) => call[1].body)).toEqual(["First", "Second"]);
		expect(readArtifact).toHaveBeenCalledTimes(4);
	});
});

describe("registerForgeTools edit contracts", () => {
	it("edits artifact title and body exactly while returning only a compact verified summary", async () => {
		const original = issue({ title: "Parser & failure", body: "alpha beta\r\nliteral <tag>" });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = issue({
				...current,
				title: input.title ?? current.title,
				body: input.body ?? current.body,
			});
			return { kind: "issue", number: 7, url: current.url };
		});
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const receipts = [
			...fieldReceipts(original, { kind: "artifact" }, "title"),
			...fieldReceipts(original, { kind: "artifact" }, "body"),
		];
		const branch = evidenceBranch(receipts, "read_issue", [{ id: "edit", name: "edit_issue" }]);

		const result = await editIssueTool.execute(
			"edit",
			{
				number: 7,
				target: { kind: "artifact" },
				edits: [
					{ field: "title", oldText: "Parser &", newText: "Strict <parser>" },
					{ field: "body", oldText: "alpha", newText: "beta" },
					{ field: "body", oldText: "beta", newText: "gamma" },
					{ field: "body", oldText: "<tag>", newText: "<node>" },
				],
			},
			undefined,
			undefined,
			toolContext(branch),
		);

		expect(readArtifact).toHaveBeenCalledTimes(2);
		expect(updateArtifact).toHaveBeenCalledTimes(1);
		expect(updateArtifact).toHaveBeenCalledWith(
			githubRepository,
			{
				kind: "issue",
				number: 7,
				title: "Strict <parser> failure",
				body: "beta gamma\r\nliteral <node>",
			},
			undefined,
		);
		expect(result.content[0].text).toBe(original.url);
		expect(Buffer.byteLength(result.content[0].text, "utf8")).toBeLessThanOrEqual(512);
		expect(JSON.stringify(result)).not.toMatch(/Strict <parser>|<node>|alpha|gamma/);
		expect(result.details).toMatchObject({
			schema: "scramjet:forge-mutation@1",
			operation: "edit_issue",
			identity: { kind: "issue", number: 7, url: original.url },
			target: { kind: "artifact" },
			fields: ["title", "body"],
			replacements: 4,
			verified: true,
		});
	});

	it("distinguishes decoded controls from their visible read representation during edits", async () => {
		const original = issue({ body: "A\tB" });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = issue({ ...current, body: input.body ?? current.body });
			return { kind: "issue", number: 7, url: current.url };
		});
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const receipt = fullReceipt(original);

		const rejectedBranch = evidenceBranch([receipt], "read_issue", [{ id: "encoded-edit", name: "edit_issue" }]);
		await expectForgeFailure(
			editIssueTool.execute(
				"encoded-edit",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "body", oldText: "^!0009;", newText: " " }],
				},
				undefined,
				undefined,
				toolContext(rejectedBranch),
			),
			"FORGE_PREFLIGHT_FAILED",
			/not found exactly/i,
		);
		expect(updateArtifact).not.toHaveBeenCalled();

		const decodedBranch = evidenceBranch([receipt], "read_issue", [{ id: "decoded-edit", name: "edit_issue" }]);
		await editIssueTool.execute(
			"decoded-edit",
			{
				number: 7,
				target: { kind: "artifact" },
				edits: [{ field: "body", oldText: "\t", newText: " " }],
			},
			undefined,
			undefined,
			toolContext(decodedBranch),
		);
		expect(updateArtifact).toHaveBeenCalledWith(
			githubRepository,
			{ kind: "issue", number: 7, body: "A B" },
			undefined,
		);
	});

	it("edits a literal escape spelling without decoding it", async () => {
		const original = issue({ body: "literal ^!0009;" });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = issue({ ...current, body: input.body ?? current.body });
			return { kind: "issue", number: 7, url: current.url };
		});
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [
			{ id: "literal-edit", name: "edit_issue" },
		]);
		await editIssueTool.execute(
			"literal-edit",
			{
				number: 7,
				target: { kind: "artifact" },
				edits: [{ field: "body", oldText: "^!0009;", newText: "kept literal" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);
		expect(updateArtifact).toHaveBeenCalledWith(
			githubRepository,
			{ kind: "issue", number: 7, body: "literal kept literal" },
			undefined,
		);
	});

	it("edits one comment from complete same-snapshot range coverage without authorizing another object", async () => {
		const body = `${"x".repeat(600)} & tail`;
		const original = issue({ comments: [{ ...issue().comments[0], body }] });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateComment = vi.fn<ForgeAdapter["updateComment"]>(async (_repository, input) => {
			current = issue({
				...current,
				comments: current.comments.map((comment) =>
					comment.id === input.id ? { ...comment, body: input.body } : comment,
				),
			});
			return { kind: "comment", id: input.id, url: current.comments[0].url };
		});
		const { editIssueTool, adapter } = toolSetup(original, { readArtifact, updateComment });
		const receipts = fieldReceipts(original, { kind: "comment", id: "101" }, "body");
		expect(receipts.length).toBeGreaterThan(1);
		const branch = evidenceBranch(receipts, "read_issue", [{ id: "edit-comment", name: "edit_issue" }]);

		const result = await editIssueTool.execute(
			"edit-comment",
			{
				number: 7,
				target: { kind: "comment", id: "101" },
				edits: [{ field: "body", oldText: " & tail", newText: " <done>" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);

		expect(updateComment).toHaveBeenCalledWith(
			githubRepository,
			{ kind: "issue", number: 7, id: "101", body: `${"x".repeat(600)} <done>` },
			undefined,
		);
		expect(adapter.updateArtifact).not.toHaveBeenCalled();
		expect(result.content[0].text).toBe(current.comments[0].url);
		expect(JSON.stringify(result)).not.toContain("<done>");
		expect(result.details).toMatchObject({
			operation: "edit_issue",
			identity: { kind: "comment", id: "101" },
			target: { kind: "comment", id: "101" },
			fields: ["body"],
			replacements: 1,
		});
	});

	it("rejects a title edit on a comment before repository or adapter work", async () => {
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>();
		const updateComment = vi.fn<ForgeAdapter["updateComment"]>();
		const { bag, editIssueTool } = toolSetup(issue(), { readArtifact, updateComment });

		await expectForgeFailure(
			editIssueTool.execute(
				"invalid-comment-edit",
				{
					number: 7,
					target: { kind: "comment", id: "101" },
					edits: [{ field: "title", oldText: "old", newText: "new" }],
				},
				undefined,
				undefined,
				toolContext(),
			),
			"FORGE_PREFLIGHT_FAILED",
			/comment edits.*body/i,
		);
		expect(bag.pi.exec).not.toHaveBeenCalled();
		expect(readArtifact).not.toHaveBeenCalled();
		expect(updateComment).not.toHaveBeenCalled();
	});

	it("executes edit_pr with read_pr evidence and PR adapter identity", async () => {
		const original = pullRequest({ body: "Before PR" });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = pullRequest({ ...current, body: input.body ?? current.body });
			return { kind: "pr", number: 12, url: current.url };
		});
		const { editPrTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(original)], "read_pr", [{ id: "edit-pr", name: "edit_pr" }]);

		const result = await editPrTool.execute(
			"edit-pr",
			{
				number: 12,
				target: { kind: "artifact" },
				edits: [{ field: "body", oldText: "Before PR", newText: "After PR" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);

		expect(readArtifact).toHaveBeenNthCalledWith(1, githubRepository, "pr", 12, [], undefined);
		expect(updateArtifact).toHaveBeenCalledWith(
			githubRepository,
			{ kind: "pr", number: 12, body: "After PR" },
			undefined,
		);
		expect(result.details).toMatchObject({
			operation: "edit_pr",
			identity: { kind: "pr", number: 12 },
			fields: ["body"],
			replacements: 1,
		});
		expect(result.content[0].text).not.toContain("After PR");
	});

	it.each([
		["adds Draft", false, "Release", "Draft: Release"],
		["adds WIP", false, "Release", "WIP: Release"],
		["removes WIP", true, "WIP: Release", "Release"],
	] as const)("rejects GitLab pull request title edits that %s draft state", async (_name, draft, title, newTitle) => {
		const original = pullRequest({
			url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12",
			title,
			readiness: { ...pullRequest().readiness, draft },
		});
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>();
		const { editPrTool } = toolSetup(original, { readArtifact, updateArtifact }, gitlabRepository);
		const branch = evidenceBranch(
			[fieldReceipts(original, { kind: "artifact" }, "title", gitlabRepository)[0]],
			"read_pr",
			[{ id: "edit-pr", name: "edit_pr" }],
		);

		await expectForgeFailure(
			editPrTool.execute(
				"edit-pr",
				{
					number: 12,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: title, newText: newTitle }],
				},
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_PREFLIGHT_FAILED",
			/preserve the existing draft state/i,
		);
		expect(updateArtifact).not.toHaveBeenCalled();
	});

	it("treats unexpected GitLab draft-state drift after a title edit as ambiguous", async () => {
		const original = pullRequest({
			url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12",
			title: "Old release",
		});
		const updated = pullRequest({
			...original,
			title: "New release",
			readiness: { ...original.readiness, draft: true },
		});
		const readArtifact = vi
			.fn<ForgeAdapter["readArtifact"]>()
			.mockResolvedValueOnce(original)
			.mockResolvedValueOnce(updated);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async () => ({
			kind: "pr",
			number: 12,
			url: original.url,
		}));
		const { editPrTool } = toolSetup(original, { readArtifact, updateArtifact }, gitlabRepository);
		const branch = evidenceBranch(
			[fieldReceipts(original, { kind: "artifact" }, "title", gitlabRepository)[0]],
			"read_pr",
			[{ id: "edit-pr", name: "edit_pr" }],
		);

		await expectForgeFailure(
			editPrTool.execute(
				"edit-pr",
				{
					number: 12,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "Old", newText: "New" }],
				},
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(updateArtifact).toHaveBeenCalledTimes(1);
	});

	it("keeps a large verified postimage out of the mutation result", async () => {
		const original = issue({ title: "Old title", body: "x".repeat(100_000) });
		let current = original;
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = issue({ ...current, title: input.title ?? current.title });
			return { kind: "issue", number: 7, url: current.url };
		});
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch(fieldReceipts(original, { kind: "artifact" }, "title"), "read_issue", [
			{ id: "edit-large", name: "edit_issue" },
		]);
		const result = await editIssueTool.execute(
			"edit-large",
			{
				number: 7,
				target: { kind: "artifact" },
				edits: [{ field: "title", oldText: "Old", newText: "New" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);
		expect(Buffer.byteLength(result.content[0].text, "utf8")).toBeLessThanOrEqual(512);
		expect(JSON.stringify(result)).not.toContain("x".repeat(1000));
		expect(result.details).not.toHaveProperty("postimage");
	});

	it.each([
		{
			name: "has no prior read",
			branch: (_full: any, _parts: any[]) => evidenceBranch([], "read_issue", [{ id: "edit", name: "edit_issue" }]),
		},
		{
			name: "uses a read from the same assistant batch",
			branch: (full: any, _parts: any[]) => [
				assistantEntry([
					{ id: "same-batch-read", name: "read_issue" },
					{ id: "edit", name: "edit_issue" },
				]),
				readResultEntry("same-batch-read", "read_issue", full),
			],
		},
		{
			name: "uses evidence before compaction",
			branch: (full: any, _parts: any[]) => [
				assistantEntry([{ id: "old-read", name: "read_issue" }]),
				readResultEntry("old-read", "read_issue", full),
				branchEntry("compaction", { summary: "compact", firstKeptEntryId: "entry-1", tokensBefore: 1 }),
				assistantEntry([{ id: "edit", name: "edit_issue" }]),
			],
		},
		{
			name: "uses a failed read result",
			branch: (full: any, _parts: any[]) => [
				assistantEntry([{ id: "failed-read", name: "read_issue" }]),
				readResultEntry("failed-read", "read_issue", full, true),
				assistantEntry([{ id: "edit", name: "edit_issue" }]),
			],
		},
		{
			name: "uses evidence for another repository",
			branch: (full: any, _parts: any[]) =>
				evidenceBranch(
					[{ ...full, repository: { ...full.repository, projectPath: "Other/widget" } }],
					"read_issue",
					[{ id: "edit", name: "edit_issue" }],
				),
		},
		{
			name: "uses evidence for another artifact",
			branch: (full: any, _parts: any[]) =>
				evidenceBranch([{ ...full, artifact: { kind: "pr", number: 7 } }], "read_issue", [
					{ id: "edit", name: "edit_issue" },
				]),
		},
		{
			name: "has only partial field coverage",
			branch: (_full: any, parts: any[]) =>
				evidenceBranch([parts[0]], "read_issue", [{ id: "edit", name: "edit_issue" }]),
		},
		{
			name: "mixes partial ranges from different snapshots",
			branch: (_full: any, parts: any[]) =>
				evidenceBranch(
					parts.map((part, index) => ({ ...part, snapshot: index === 0 ? "a".repeat(64) : "b".repeat(64) })),
					"read_issue",
					[{ id: "edit", name: "edit_issue" }],
				),
		},
	])("rejects an edit that $name before any remote refetch or write", async ({ branch }) => {
		const original = issue({ body: `${"A".repeat(1199)}Z` });
		const full = fullReceipt(original);
		const parts = fieldReceipts(original, { kind: "artifact" }, "body");
		expect(parts.length).toBeGreaterThan(1);
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>();
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });

		await expectForgeFailure(
			editIssueTool.execute(
				"edit",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "body", oldText: "AZ", newText: "A!" }],
				},
				undefined,
				undefined,
				toolContext(branch(full, parts)),
			),
			"FORGE_PREFLIGHT_FAILED",
			/complete prior read_issue.*body/i,
		);
		expect(readArtifact).not.toHaveBeenCalled();
		expect(updateArtifact).not.toHaveBeenCalled();
	});

	it("classifies a queue-time refetch failure as no-write preflight", async () => {
		const original = issue({ body: "Before" });
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => {
			throw new ForgeCommandError("failed", {
				command: "gh",
				args: ["api", "repos/Acme/widget/issues/7"],
				cwd: "/repo",
				process: { exitCode: 1, stdout: "", stderr: "HTTP 503" },
			});
		});
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>();
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [{ id: "edit", name: "edit_issue" }]);

		const result = await expectForgeFailure(
			editIssueTool.execute(
				"edit",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "body", oldText: "Before", newText: "After" }],
				},
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_PREFLIGHT_FAILED",
			/no write was attempted/i,
		);
		expect(result.details).toMatchObject({ phase: "refetch", writeState: "not_attempted" });
		expect(updateArtifact).not.toHaveBeenCalled();
	});

	it("rejects fuzzy or escaped replacement text after refetch without mutating", async () => {
		const original = issue({ title: "smart—dash & literal", body: "unchanged" });
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => original);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>();
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [{ id: "edit", name: "edit_issue" }]);

		await expectForgeFailure(
			editIssueTool.execute(
				"edit",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "smart-dash &amp; literal", newText: "changed" }],
				},
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_PREFLIGHT_FAILED",
			/not found exactly/i,
		);
		expect(readArtifact).toHaveBeenCalledTimes(1);
		expect(updateArtifact).not.toHaveBeenCalled();
	});

	it("reports a possibly successful edit when identity or full mutable postimage verification fails", async () => {
		const original = issue({ title: "Original", body: "Before" });
		const readArtifact = vi
			.fn<ForgeAdapter["readArtifact"]>()
			.mockResolvedValueOnce(original)
			.mockResolvedValueOnce(issue({ ...original, title: "Changed externally", body: "After" }));
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async () => ({
			kind: "issue",
			number: 7,
			url: original.url,
		}));
		const { editIssueTool } = toolSetup(original, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [{ id: "edit", name: "edit_issue" }]);

		await expectForgeFailure(
			editIssueTool.execute(
				"edit",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "body", oldText: "Before", newText: "After" }],
				},
				undefined,
				undefined,
				toolContext(branch),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/i,
		);
		expect(updateArtifact).toHaveBeenCalledTimes(1);
		expect(readArtifact).toHaveBeenCalledTimes(2);
	});

	it("shares the parent-object queue with comment creation and releases it after failure", async () => {
		const original = issue({ body: "A" });
		let current = original;
		let releaseComment!: () => void;
		const pending = new Promise<void>((resolve) => {
			releaseComment = resolve;
		});
		const failure = new ForgeCommandError("failed", { command: "gh", args: ["api"], cwd: "/repo" });
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () => current);
		const addComment = vi.fn<ForgeAdapter["addComment"]>(async () => {
			await pending;
			throw failure;
		});
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = issue({ ...current, body: input.body ?? current.body });
			return { kind: "issue", number: 7, url: current.url };
		});
		const { addIssueCommentTool, editIssueTool } = toolSetup(original, {
			readArtifact,
			addComment,
			updateArtifact,
		});
		const branch = evidenceBranch([fullReceipt(original)], "read_issue", [
			{ id: "comment", name: "add_issue_comment" },
			{ id: "edit", name: "edit_issue" },
		]);

		const comment = addIssueCommentTool.execute(
			"comment",
			{ number: 7, body: "Comment" },
			undefined,
			undefined,
			toolContext(branch),
		);
		await vi.waitFor(() => expect(addComment).toHaveBeenCalledTimes(1));
		const edit = editIssueTool.execute(
			"edit",
			{ number: 7, target: { kind: "artifact" }, edits: [{ field: "body", oldText: "A", newText: "C" }] },
			undefined,
			undefined,
			toolContext(branch),
		);
		await flush();
		expect(readArtifact).toHaveBeenCalledTimes(1);
		expect(updateArtifact).not.toHaveBeenCalled();

		releaseComment();
		await expectForgeFailure(comment, "FORGE_WRITE_AMBIGUOUS", /DO NOT RETRY/i);
		await expect(edit).resolves.toMatchObject({ details: { verified: true } });
		expect(updateArtifact).toHaveBeenCalledTimes(1);
		expect(readArtifact).toHaveBeenCalledTimes(3);
	});

	it("allows edits to different remote objects to proceed concurrently", async () => {
		const firstIssue = issue({ number: 7, body: "Seven" });
		const secondIssue = issue({
			number: 8,
			url: "https://github.com/Acme/widget/issues/8",
			body: "Eight",
		});
		const current = new Map<number, ForgeIssue>([
			[7, firstIssue],
			[8, secondIssue],
		]);
		let releaseFirst!: () => void;
		const pending = new Promise<void>((resolve) => {
			releaseFirst = resolve;
		});
		const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async (_repository, _kind, number) => {
			const artifact = current.get(number);
			if (artifact === undefined) throw new Error(`Missing issue ${number}`);
			return artifact;
		});
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			if (input.number === 7) await pending;
			const artifact = current.get(input.number);
			if (artifact === undefined) throw new Error(`Missing issue ${input.number}`);
			const updated = issue({ ...artifact, body: input.body ?? artifact.body });
			current.set(input.number, updated);
			return { kind: "issue", number: input.number, url: updated.url };
		});
		const { editIssueTool } = toolSetup(firstIssue, { readArtifact, updateArtifact });
		const branch = evidenceBranch([fullReceipt(firstIssue), fullReceipt(secondIssue)], "read_issue", [
			{ id: "seven", name: "edit_issue" },
			{ id: "eight", name: "edit_issue" },
		]);

		const first = editIssueTool.execute(
			"seven",
			{
				number: 7,
				target: { kind: "artifact" },
				edits: [{ field: "body", oldText: "Seven", newText: "Updated seven" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);
		await vi.waitFor(() => expect(updateArtifact).toHaveBeenCalledTimes(1));
		const second = editIssueTool.execute(
			"eight",
			{
				number: 8,
				target: { kind: "artifact" },
				edits: [{ field: "body", oldText: "Eight", newText: "Updated eight" }],
			},
			undefined,
			undefined,
			toolContext(branch),
		);

		await expect(second).resolves.toMatchObject({ details: { identity: { number: 8 } } });
		expect(first).not.toBeUndefined();
		expect(updateArtifact).toHaveBeenCalledTimes(2);
		releaseFirst();
		await expect(first).resolves.toMatchObject({ details: { identity: { number: 7 } } });
	});
});

describe("forge startup prerequisites", () => {
	function startupSetup(
		remote: string,
		authResult: ExecResult | Promise<ExecResult>,
		options: { hasUI?: boolean; rejectGit?: boolean } = {},
	) {
		const bag = recordingPi();
		bag.pi.exec = vi.fn(async (command: string, _args: string[]) => {
			if (command === "git") {
				if (options.rejectGit) throw new Error("disposed");
				return execResult({ stdout: remote });
			}
			return authResult;
		});
		const notify = vi.fn();
		const warn = vi.fn();
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(vi.fn()), logger: { warn } });
		return {
			bag,
			notify,
			warn,
			ctx: { cwd: "/repo", hasUI: options.hasUI ?? true, ui: { notify } },
		};
	}

	it.each([
		{
			name: "GitHub",
			remote: "git@github.com:Acme/widget.git\n",
			command: "gh",
			host: "github.com",
		},
		{
			name: "GitLab",
			remote: "https://gitlab.com/Acme/platform/widget.git\n",
			command: "glab",
			host: "gitlab.com",
		},
	])("checks only the selected CLI for a $name repository", async ({ remote, command, host }) => {
		const { bag, notify, ctx } = startupSetup(remote, execResult());
		await bag.emit("session_start", {}, ctx);
		await flush();

		expect(bag.pi.exec.mock.calls.map((call: unknown[]) => call[0])).toEqual(["git", command]);
		expect(bag.pi.exec.mock.calls[1]).toEqual([
			command,
			["auth", "status", "--hostname", host, ...(command === "gh" ? ["--active"] : [])],
			{ cwd: "/repo", timeout: 3000 },
		]);
		expect(notify).not.toHaveBeenCalled();
	});

	it("ignores invalid inactive GitHub accounts when the active account is usable", async () => {
		const bag = recordingPi();
		bag.pi.exec = vi.fn(async (command: string, args: string[]) => {
			if (command === "git") return execResult({ stdout: "https://github.com/Acme/widget.git" });
			return args.includes("--active")
				? execResult()
				: execResult({ code: 1, stderr: "An inactive account token is invalid" });
		});
		const notify = vi.fn();
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(vi.fn()) });
		await bag.emit("session_start", {}, { cwd: "/repo", hasUI: true, ui: { notify } });
		await flush();

		expect(bag.pi.exec.mock.calls[1][1]).toContain("--active");
		expect(notify).not.toHaveBeenCalled();
	});

	it("warns when the selected CLI executable is conclusively missing", async () => {
		const { bag, notify, ctx } = startupSetup(
			"https://gitlab.com/Acme/widget.git",
			execResult({ code: 1, spawnError: { code: "ENOENT", message: "spawn glab ENOENT" } }),
		);
		await bag.emit("session_start", {}, ctx);
		await flush();

		expect(notify).toHaveBeenCalledWith(expect.stringMatching(/GitLab CLI \(glab\).*not installed/i), "warning");
		expect(bag.pi.exec.mock.calls.some((call: unknown[]) => call[0] === "gh")).toBe(false);
	});

	it.each([
		"You are not logged into any GitHub hosts. Run gh auth login to authenticate.",
		"The token in GH_TOKEN is invalid.",
		"API request failed with HTTP 401 Unauthorized",
	])("warns with login guidance for explicit authentication failure: %s", async (diagnostic) => {
		const { bag, notify, ctx } = startupSetup(
			"https://github.com/Acme/widget.git",
			execResult({ code: 1, stderr: diagnostic }),
		);
		await bag.emit("session_start", {}, ctx);
		await flush();

		expect(notify).toHaveBeenCalledWith(expect.stringContaining("gh auth login --hostname github.com"), "warning");
	});

	it.each([
		execResult({ code: 1, stderr: "dial tcp: network is unreachable" }),
		execResult({ code: 1, stderr: "insufficient scope to inspect organization" }),
		execResult({ killed: true }),
		execResult({ code: 1, spawnError: { code: "EACCES", message: "permission denied" } }),
	])("stays silent for an inconclusive or transient selected-CLI failure", async (authResult) => {
		const { bag, notify, ctx } = startupSetup("https://github.com/Acme/widget.git", authResult);
		await bag.emit("session_start", {}, ctx);
		await flush();
		expect(notify).not.toHaveBeenCalled();
	});

	it("does no work in headless mode and stays silent for unsupported or unavailable repositories", async () => {
		const headless = startupSetup("https://github.com/Acme/widget.git", execResult(), { hasUI: false });
		await headless.bag.emit("session_start", {}, headless.ctx);
		expect(headless.bag.pi.exec).not.toHaveBeenCalled();
		expect(headless.warn).not.toHaveBeenCalled();

		const unsupported = startupSetup("git@github-work:Acme/widget.git", execResult());
		await unsupported.bag.emit("session_start", {}, unsupported.ctx);
		await flush();
		expect(unsupported.bag.pi.exec).toHaveBeenCalledTimes(1);
		expect(unsupported.notify).not.toHaveBeenCalled();
		expect(unsupported.warn).not.toHaveBeenCalled();

		const unavailable = startupSetup("", execResult(), { rejectGit: true });
		await unavailable.bag.emit("session_start", {}, unavailable.ctx);
		await flush();
		expect(unavailable.notify).not.toHaveBeenCalled();
		expect(unavailable.warn).not.toHaveBeenCalled();
	});

	it("journals unexpected detached startup probe defects", async () => {
		const bag = recordingPi();
		const warn = vi.fn();
		registerForgeTools(bag.pi, {
			createAdapter: () => adapterFor(vi.fn()),
			resolveRepository: async () => {
				throw new Error("resolver defect");
			},
			logger: { warn },
		});
		await bag.emit("session_start", {}, { cwd: "/repo", hasUI: true, ui: { notify: vi.fn() } });
		await flush();
		expect(warn).toHaveBeenCalledWith("forge", "startup prerequisite probe failed", {
			error: "Error: resolver defect",
		});
	});

	it("detaches the probe from session_start and journals notification defects", async () => {
		let resolveGit!: (result: ExecResult) => void;
		const gitPending = new Promise<ExecResult>((resolve) => {
			resolveGit = resolve;
		});
		const bag = recordingPi();
		bag.pi.exec = vi.fn((command: string) =>
			command === "git"
				? gitPending
				: Promise.resolve(execResult({ spawnError: { code: "ENOENT", message: "missing" } })),
		);
		const notify = vi.fn(() => {
			throw new Error("stale UI");
		});
		const warn = vi.fn();
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(vi.fn()), logger: { warn } });

		await expect(
			bag.emit("session_start", {}, { cwd: "/repo", hasUI: true, ui: { notify } }),
		).resolves.toBeUndefined();
		resolveGit(execResult({ stdout: "https://github.com/Acme/widget.git" }));
		await flush();
		expect(warn).toHaveBeenCalledWith("forge", "startup prerequisite probe failed", {
			error: "Error: stale UI",
		});
	});
});
