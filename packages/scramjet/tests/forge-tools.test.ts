import { mkdtempSync, rmSync } from "node:fs";
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
import { renderForgeDocument } from "../src/forge/document.js";
import { registerForgeTools } from "../src/forge/tools.js";
import type { ForgeAdapter, ForgeArtifact, ForgeIssue, ForgePullRequest, ForgeRepository } from "../src/forge/types.js";
import { recordingPi } from "./helpers.js";

const githubRepository: ForgeRepository = {
	forge: "github",
	host: "github.com",
	projectPath: "Acme/widget",
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
			reviewDecision: "approved",
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

function adapterFor(readArtifact: ForgeAdapter["readArtifact"]): ForgeAdapter {
	return {
		readArtifact,
		createArtifact: vi.fn(),
		updateArtifact: vi.fn(),
		addComment: vi.fn(),
		updateComment: vi.fn(),
	};
}

function toolSetup(artifact: ForgeArtifact | (() => ForgeArtifact)) {
	const bag = recordingPi();
	bag.pi.exec = vi.fn(async (command: string, args: string[]) => {
		if (command === "git" && args.join(" ") === "remote get-url origin") {
			return execResult({ stdout: "https://github.com/Acme/widget.git\n" });
		}
		throw new Error(`Unexpected command: ${command} ${args.join(" ")}`);
	});
	const readArtifact = vi.fn<ForgeAdapter["readArtifact"]>(async () =>
		typeof artifact === "function" ? artifact() : artifact,
	);
	const adapter = adapterFor(readArtifact);
	registerForgeTools(bag.pi, { createAdapter: () => adapter });
	return {
		bag,
		adapter,
		readArtifact,
		issueTool: bag.tools.find((tool) => tool.name === "read_issue"),
		prTool: bag.tools.find((tool) => tool.name === "read_pr"),
	};
}

function toolContext() {
	return { cwd: "/repo" };
}

function theme() {
	return {
		fg: (_color: string, value: string) => value,
		bold: (value: string) => value,
	};
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("registerForgeTools read contracts", () => {
	it("registers exactly two independently named, strict model-callable read tools", () => {
		const { bag, issueTool, prTool } = toolSetup(issue());
		expect(bag.tools.map((tool) => tool.name)).toEqual(["read_issue", "read_pr"]);
		for (const tool of [issueTool, prTool]) {
			expect(tool.activation).toBeUndefined();
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
	});

	it("fetches and validates the aggregate issue before returning canonical XML with trusted details", async () => {
		const { issueTool, readArtifact } = toolSetup(issue());
		const controller = new AbortController();
		const result = await issueTool.execute("read-1", { number: 7 }, controller.signal, undefined, toolContext());

		expect(readArtifact).toHaveBeenCalledWith(githubRepository, "issue", 7, [], controller.signal);
		expect(result.content[0].text).toContain('<forge-artifact version="1" forge="github"');
		expect(result.content[0].text).toContain('<comment id="101"');
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
		expect(text.indexOf("<comments>")).toBeLessThan(text.indexOf("<files>"));
		expect(text.indexOf("<files>")).toBeLessThan(text.indexOf("<checks>"));
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
		expect(first.content[0].text).toContain('include=["files"] to continue');

		await expect(
			tool.execute(
				"pr-range-2",
				{ number: 12, include: ["files"], offset: 2, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
		).resolves.toMatchObject({ details: { snapshot: first.details.snapshot } });
		await expect(
			tool.execute(
				"pr-range-3",
				{ number: 12, offset: 2, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
		).rejects.toThrow(/snapshot changed/i);
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
		expect(first.content[0].text).toContain(`snapshot=${first.details.snapshot}`);

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
		await expect(
			issueTool.execute(
				"range-3",
				{ number: 7, offset: bodyLine + 5, snapshot: first.details.snapshot },
				undefined,
				undefined,
				toolContext(),
			),
		).rejects.toThrow(/snapshot changed/i);
		expect(readArtifact).toHaveBeenCalledTimes(3);
	});

	it("persists trusted read details through the real AgentSession tool pipeline", async () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-forge-tool-"));
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		const tools: ToolDefinition[] = [];
		const adapter = adapterFor(async () => issue());
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
				noTools: "builtin",
			}));
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
		} finally {
			session?.dispose();
			rmSync(root, { recursive: true, force: true });
		}
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
		expect(collapsed.render(120).join("\n")).toMatch(/lines 1-\d+ of \d+/);

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
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(vi.fn()) });
		return {
			bag,
			notify,
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

		const unsupported = startupSetup("git@github-work:Acme/widget.git", execResult());
		await unsupported.bag.emit("session_start", {}, unsupported.ctx);
		await flush();
		expect(unsupported.bag.pi.exec).toHaveBeenCalledTimes(1);
		expect(unsupported.notify).not.toHaveBeenCalled();

		const unavailable = startupSetup("", execResult(), { rejectGit: true });
		await unavailable.bag.emit("session_start", {}, unavailable.ctx);
		await flush();
		expect(unavailable.notify).not.toHaveBeenCalled();
	});

	it("detaches the caught probe from session_start", async () => {
		let resolveGit!: (result: ExecResult) => void;
		const gitPending = new Promise<ExecResult>((resolve) => {
			resolveGit = resolve;
		});
		const bag = recordingPi();
		bag.pi.exec = vi.fn(() => gitPending);
		const notify = vi.fn(() => {
			throw new Error("stale UI");
		});
		registerForgeTools(bag.pi, { createAdapter: () => adapterFor(vi.fn()) });

		await expect(
			bag.emit("session_start", {}, { cwd: "/repo", hasUI: true, ui: { notify } }),
		).resolves.toBeUndefined();
		resolveGit(execResult({ stdout: "https://github.com/Acme/widget.git" }));
		await flush();
	});
});
