import { writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { initTheme } from "@leanandmean/coding-agent";
import { visibleWidth } from "@leanandmean/tui";
import { describe, expect, it, vi } from "vitest";
import { resetCache } from "../src/autonomy-settings.js";
import { registerForgePublication } from "../src/forge-publication.js";
import { freshState, recordingPi } from "./helpers.js";

initTheme(undefined, false);

function execResult(stdout = "", code = 0) {
	return { stdout, stderr: "", code, killed: false };
}

function context(custom?: (factory: any, options?: any) => Promise<any>) {
	return {
		hasUI: Boolean(custom),
		cwd: "/repo",
		ui: custom ? { custom } : undefined,
	};
}

function allowPublication(state: ReturnType<typeof freshState>, command: string, tools: string[]): void {
	state.registry = new Map([
		[command, { name: command, filePath: `/commands/${command}.md`, body: "", allowedTools: tools }],
	]);
}

async function registered() {
	const bag = recordingPi();
	bag.pi.exec = vi.fn().mockResolvedValue(execResult("https://github.com/LeanAndMean/scramjet.git\n"));
	const state = freshState();
	registerForgePublication(bag.pi, state);
	return { ...bag, state, tool: bag.tools.find((candidate) => candidate.name === "create_issue") };
}

describe("create_issue approval", () => {
	it("renders compact facts and reconstructs the expanded proposal from call arguments", async () => {
		const { tools } = await registered();
		const tool = tools.find((candidate) => candidate.name === "create_pr");
		const args = { title: "Exact title", body: "Exact body", head: "feature", base: "main", draft: true };
		const compact = tool.renderCall(args, theme(), { expanded: false }).render(120).join("\n");
		const expanded = tool.renderCall(args, theme(), { expanded: true }).render(120).join("\n");
		expect(compact).toContain("feature → main, draft");
		expect(compact).not.toContain("Exact body");
		const hostile = tool
			.renderCall({ ...args, title: "unsafe\u001b]8;;x\u0007\u202e" }, theme(), { expanded: false })
			.render(120)
			.join("\n");
		expect(hostile).not.toMatch(/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u);
		expect(expanded).toContain("Exact title");
		expect(expanded).toContain("Exact body");
		expect(expanded).toContain("feature");
		expect(expanded).toContain("main");
	});

	it("uses the complete expanded payload as the committed preview for all four operations", async () => {
		const longBody = [
			"BEGIN",
			...Array.from({ length: 30 }, (_, index) => `BODY-LINE-${index.toString().padStart(2, "0")}`),
			"END\u001b]8;;x\u0007",
		].join("\n");
		const cases = [
			{
				name: "create_issue",
				args: { title: "Issue title", body: longBody },
				expected: [
					"Issue title",
					"BEGIN",
					...Array.from({ length: 30 }, (_, index) => `BODY-LINE-${index.toString().padStart(2, "0")}`),
					"END",
				],
				order: ["Title", "Issue title", "Body", "BEGIN", "BODY-LINE-29", "END"],
			},
			{
				name: "create_pr",
				args: { title: "PR title", body: "PR body", head: "feature", base: "main", draft: true },
				expected: ["PR title", "PR body", "feature", "main", "true"],
				order: ["Title", "PR title", "Head", "feature", "Base", "main", "Draft", "true", "Body", "PR body"],
			},
			{
				name: "add_issue_comment",
				args: { number: 41, body: "Issue comment" },
				expected: ["#41", "Issue comment"],
				order: ["Target", "#41", "Comment", "Issue comment"],
			},
			{
				name: "add_pr_comment",
				args: { number: 42, body: "PR comment" },
				expected: ["#42", "PR comment"],
				order: ["Target", "#42", "Comment", "PR comment"],
			},
		] as const;
		for (const testCase of cases) {
			let preview = "";
			const custom = async (factory: any, options?: any) => {
				let answer: unknown;
				const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
					answer = value;
				});
				preview = options.toolAttachedContext
					.render({ requestRender() {} }, theme())
					.render(70)
					.join("\n");
				component.handleInput("\u001b");
				return answer;
			};
			const { tools, pi } = await registered();
			const tool = tools.find((candidate) => candidate.name === testCase.name);
			await tool.execute("call", testCase.args, undefined, undefined, context(custom));

			for (const value of testCase.expected) expect(preview).toContain(value);
			let previous = -1;
			for (const value of testCase.order) {
				const current = preview.indexOf(value, previous + 1);
				expect(current, value).toBeGreaterThan(previous);
				previous = current;
			}
			expect(preview).toContain("Provider: github");
			expect(preview).toContain("Repository: LeanAndMean/scramjet");
			expect(preview).toContain("Consequence:");
			expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
			expect(preview).not.toContain("\u001b]8;;x");
			expect(preview).not.toMatch(
				/[\u0000-\u0008\u000b-\u001a\u001c-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u,
			);
		}
	});

	it("renders complete default-Approve controls at the practical narrow width", async () => {
		let lines: string[] = [];
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			lines = component.render(23);
			component.handleInput("\u001b");
			return answer;
		};
		const { tool } = await registered();
		await tool.execute("call", { title: "hidden title", body: "hidden body" }, undefined, undefined, context(custom));

		const rendered = lines.join("\n");
		expect(rendered).toContain("→ Approve publication");
		expect(rendered.indexOf("Approve publication")).toBeLessThan(rendered.indexOf("Cancel"));
		expect(rendered).toContain("Esc cancel • ↑↓ • Enter");
		expect(rendered).not.toContain("hidden title");
		expect(rendered).not.toContain("hidden body");
		expect(rendered).not.toContain("...");
		for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(23);
		expect(rendered).not.toMatch(
			/lines (?:above|below)|Beginning of payload|End of payload|PgUp|PgDn|Tab|←|→ choose/,
		);
	});

	it("renders every settled certainty without proposal content in details", async () => {
		const { tools } = await registered();
		const tool = tools.find((candidate) => candidate.name === "create_issue");
		const proposal = { title: "secret proposal title", body: "secret proposal body" };
		const headless = await tool.execute("call", proposal, undefined, undefined, context());
		expect(JSON.stringify(headless)).not.toContain(proposal.title);
		expect(JSON.stringify(headless)).not.toContain(proposal.body);
		for (const [outcome, writeState] of [
			["verified", "verified"],
			["cancelled", "not-dispatched"],
			["pre-dispatch-failure", "not-dispatched"],
			["stale", "not-dispatched"],
			["headless", "not-dispatched"],
			["ambiguous", "possible"],
		] as const) {
			const details = {
				kind: "scramjet:forge-publication",
				operation: "create_issue",
				outcome,
				writeState,
				reason: "safe-reason",
				...(outcome === "verified" ? { url: "https://github.com/a/b/issues/1" } : {}),
			};
			const text = tool
				.renderResult({ content: [{ type: "text", text: "ignored" }], details }, {}, theme())
				.render(120)
				.join("\n");
			expect(text.length).toBeGreaterThan(10);
		}
	});

	it("registers all four independently allowlistable prompt-visible sequential tools", async () => {
		const { tools } = await registered();
		expect(
			tools
				.map((tool) => tool.name)
				.filter((name) => ["create_issue", "create_pr", "add_issue_comment", "add_pr_comment"].includes(name)),
		).toEqual(["create_issue", "create_pr", "add_issue_comment", "add_pr_comment"]);
		for (const tool of tools.filter(
			(candidate) => candidate.name.includes("issue") || candidate.name.includes("pr"),
		)) {
			expect(tool.executionMode).toBe("sequential");
			expect(tool.activation).toBeUndefined();
			expect(tool.promptSnippet).toContain(tool.name);
		}
	});

	it("preflights both PR branches before approval and rejects unprefixed GitLab drafts", async () => {
		const bag = recordingPi();
		bag.pi.exec = vi.fn().mockResolvedValueOnce(execResult("https://gitlab.com/group/project.git\n"));
		registerForgePublication(bag.pi, freshState());
		const tool = bag.tools.find((candidate) => candidate.name === "create_pr");
		const outcome = await tool.execute(
			"call",
			{ title: "PR", body: "b", head: "feature", base: "main", draft: true },
			undefined,
			undefined,
			context(async () => "approved"),
		);
		expect(outcome.details).toMatchObject({ outcome: "pre-dispatch-failure", writeState: "not-dispatched" });
		expect(bag.pi.exec).toHaveBeenCalledTimes(1);

		const githubBag = recordingPi();
		githubBag.pi.exec = vi
			.fn()
			.mockResolvedValueOnce(execResult("https://github.com/a/b.git\n"))
			.mockResolvedValueOnce(execResult())
			.mockResolvedValueOnce(execResult());
		registerForgePublication(githubBag.pi, freshState());
		const githubTool = githubBag.tools.find((candidate) => candidate.name === "create_pr");
		await githubTool.execute(
			"call",
			{ title: "PR", body: "b", head: "feature", base: "main", draft: false },
			undefined,
			undefined,
			context(async () => "cancelled"),
		);
		expect(githubBag.pi.exec.mock.calls.slice(1).map((call: any[]) => call[1])).toEqual([
			["show-ref", "--verify", "--quiet", "refs/remotes/origin/feature"],
			["show-ref", "--verify", "--quiet", "refs/remotes/origin/main"],
		]);
		expect(githubBag.pi.exec.mock.calls.some((call: any[]) => call[1]?.includes("POST"))).toBe(false);
	});

	it("uses an active command default to skip only the approval UI", async () => {
		const { tool, pi, state } = await registered();
		state.lifecycle.activeCommand = "mach12:issue-create";
		allowPublication(state, "mach12:issue-create", ["create_issue"]);
		state.autonomyRecommendations = new Map([
			["mach12", { edges: {}, publications: { "mach12:issue-create": { create_issue: "auto-approve" } } }],
		]);
		const custom = vi.fn(async () => {
			throw new Error("approval UI must not open");
		});
		pi.exec
			.mockResolvedValueOnce(execResult("https://github.com/LeanAndMean/scramjet.git\n"))
			.mockResolvedValueOnce(execResult("https://github.com/LeanAndMean/scramjet.git\n"))
			.mockResolvedValueOnce(
				execResult(JSON.stringify({ number: 9, html_url: "https://github.com/LeanAndMean/scramjet/issues/9" })),
			)
			.mockResolvedValueOnce(
				execResult(
					JSON.stringify({
						number: 9,
						title: "exact",
						body: "body",
						html_url: "https://github.com/LeanAndMean/scramjet/issues/9",
					}),
				),
			);

		const outcome = await tool.execute(
			"call",
			{ title: "exact", body: "body" },
			undefined,
			undefined,
			context(custom),
		);

		expect(custom).not.toHaveBeenCalled();
		expect(outcome.details).toMatchObject({
			outcome: "verified",
			authorization: { mode: "command-default", command: "mach12:issue-create" },
		});
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(1);
	});

	it.each([
		["create_issue", { title: "issue", body: "body" }],
		["create_pr", { title: "pr", body: "body", head: "feature", base: "main", draft: false }],
		["add_issue_comment", { number: 41, body: "body" }],
		["add_pr_comment", { number: 42, body: "body" }],
	] as const)("auto-approves %s headlessly through the shared guarded path", async (name, params) => {
		const { tools, pi, state } = await registered();
		state.lifecycle.activeCommand = "mach12:publish";
		allowPublication(state, "mach12:publish", [name]);
		state.autonomyRecommendations = new Map([
			["mach12", { edges: {}, publications: { "mach12:publish": { [name]: "auto-approve" } } }],
		]);
		pi.exec.mockImplementation(async (_command: string, args: string[]) =>
			args.includes("POST") ? execResult("", 1) : execResult("https://github.com/LeanAndMean/scramjet.git\n"),
		);
		const tool = tools.find((candidate) => candidate.name === name);
		const outcome = await tool.execute("call", params, undefined, undefined, context());
		expect(outcome.details).toMatchObject({
			outcome: "ambiguous",
			authorization: { mode: "command-default", command: "mach12:publish" },
		});
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(1);
	});

	it("lets an exact Always ask override supersede an auto-approving command default", async () => {
		const { tool, pi, state } = await registered();
		state.lifecycle.activeCommand = "mach12:issue-create";
		allowPublication(state, "mach12:issue-create", ["create_issue"]);
		state.autonomyRecommendations = new Map([
			["mach12", { edges: {}, publications: { "mach12:issue-create": { create_issue: "auto-approve" } } }],
		]);
		state.autonomyConfigPath = join(tmpdir(), `scramjet-ask-override-${Date.now()}.yaml`);
		writeFileSync(state.autonomyConfigPath, "publications:\n  mach12:issue-create:\n    create_issue: always-ask\n");
		resetCache();
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context());
		expect(outcome.details).toMatchObject({ outcome: "headless", writeState: "not-dispatched" });
		expect(pi.exec).not.toHaveBeenCalled();
	});

	it("falls back to headless no-write when user autonomy config is corrupt", async () => {
		const { tool, pi, state } = await registered();
		state.lifecycle.activeCommand = "mach12:issue-create";
		allowPublication(state, "mach12:issue-create", ["create_issue"]);
		state.autonomyRecommendations = new Map([
			["mach12", { edges: {}, publications: { "mach12:issue-create": { create_issue: "auto-approve" } } }],
		]);
		state.autonomyConfigPath = join(tmpdir(), `scramjet-corrupt-autonomy-${Date.now()}.yaml`);
		writeFileSync(state.autonomyConfigPath, "{ invalid: [");
		resetCache();
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context());
		expect(outcome.details).toMatchObject({ outcome: "headless", writeState: "not-dispatched" });
		expect(pi.exec).not.toHaveBeenCalled();
	});

	it("invalidates pending approval when command defaults change", async () => {
		const { tool, pi, state } = await registered();
		state.lifecycle.activeCommand = "mach12:issue-create";
		allowPublication(state, "mach12:issue-create", ["create_issue"]);
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			state.autonomyRecommendations = new Map([
				["mach12", { edges: {}, publications: { "mach12:issue-create": { create_issue: "auto-approve" } } }],
			]);
			component.handleInput("\r");
			return answer;
		};
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		expect(outcome.details.outcome).toBe("stale");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});

	it("registers one sequential ordinary tool and fails headless before mutation", async () => {
		const { tool, pi } = await registered();
		expect(tool).toMatchObject({ name: "create_issue", executionMode: "sequential" });
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context());
		expect(outcome.details).toMatchObject({
			kind: "scramjet:forge-publication",
			outcome: "headless",
			writeState: "not-dispatched",
		});
		expect(pi.exec).not.toHaveBeenCalled();
	});

	it("fails explicitly when the current UI cannot host custom approval", async () => {
		const { tool, pi } = await registered();
		const custom = vi.fn(async () => undefined);
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		expect(custom).toHaveBeenCalledOnce();
		expect(outcome.details).toMatchObject({
			outcome: "pre-dispatch-failure",
			writeState: "not-dispatched",
			reason: "interactive-approval-unavailable",
		});
		expect(JSON.stringify(outcome.content)).toContain("interactive-approval-unavailable");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});

	it("fails closed when approval UI setup rejects", async () => {
		const { tool, pi } = await registered();
		const outcome = await tool.execute(
			"call",
			{ title: "t", body: "b" },
			undefined,
			undefined,
			context(async () => {
				throw new Error("preview failed");
			}),
		);
		expect(outcome.details).toMatchObject({
			outcome: "pre-dispatch-failure",
			writeState: "not-dispatched",
		});
		expect(outcome.details.reason).toContain("approval-ui-failed");
		expect(outcome.details.reason).toContain("preview failed");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});

	it("Escape cancels and performs no mutation", async () => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput("\u001b");
			return answer;
		};
		const { tool, pi } = await registered();
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		expect(outcome.details.outcome).toBe("cancelled");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});

	it("publishes the frozen exact proposal from the initial Approve selection", async () => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput("\r");
			return answer;
		};
		const { tool, pi } = await registered();
		pi.exec
			.mockResolvedValueOnce(execResult("https://github.com/LeanAndMean/scramjet.git\n"))
			.mockResolvedValueOnce(execResult("https://github.com/LeanAndMean/scramjet.git\n"))
			.mockResolvedValueOnce(
				execResult(JSON.stringify({ number: 9, html_url: "https://github.com/LeanAndMean/scramjet/issues/9" })),
			)
			.mockResolvedValueOnce(
				execResult(
					JSON.stringify({
						number: 9,
						title: "exact",
						body: "body\r\n",
						html_url: "https://github.com/LeanAndMean/scramjet/issues/9",
					}),
				),
			);
		const params = { title: "exact", body: "body\r\n" };
		const promise = tool.execute("call", params, undefined, undefined, context(custom));
		params.title = "mutated";
		const outcome = await promise;
		expect(outcome.details).toMatchObject({
			outcome: "verified",
			url: "https://github.com/LeanAndMean/scramjet/issues/9",
		});
		expect(pi.exec.mock.calls[2]?.[2]?.stdin).toBe(JSON.stringify({ title: "exact", body: "body\r\n" }));
	});

	it.each([
		{
			name: "create_issue",
			params: { title: "issue title", body: "issue body" },
			mutated: { title: "changed", body: "changed" },
			payload: { title: "issue title", body: "issue body" },
			endpoint: "repos/LeanAndMean/scramjet/issues",
		},
		{
			name: "create_pr",
			params: { title: "PR title", body: "PR body", head: "feature", base: "main", draft: true },
			mutated: { title: "changed", body: "changed", head: "other", base: "other", draft: false },
			payload: { title: "PR title", body: "PR body", head: "feature", base: "main", draft: true },
			endpoint: "repos/LeanAndMean/scramjet/pulls",
		},
		{
			name: "add_issue_comment",
			params: { number: 41, body: "issue comment" },
			mutated: { number: 99, body: "changed" },
			payload: { body: "issue comment" },
			endpoint: "repos/LeanAndMean/scramjet/issues/41/comments",
		},
		{
			name: "add_pr_comment",
			params: { number: 42, body: "PR comment" },
			mutated: { number: 99, body: "changed" },
			payload: { body: "PR comment" },
			endpoint: "repos/LeanAndMean/scramjet/issues/42/comments",
		},
	])("freezes $name and maps one uncertain mutation to an ambiguous non-retriable result", async (testCase) => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput("\r");
			return answer;
		};
		const { tools, pi } = await registered();
		pi.exec.mockImplementation(async (_command: string, args: string[]) =>
			args.includes("POST") ? execResult("", 1) : execResult("https://github.com/LeanAndMean/scramjet.git\n"),
		);
		const tool = tools.find((candidate) => candidate.name === testCase.name);
		const promise = tool.execute("call", testCase.params, undefined, undefined, context(custom));
		Object.assign(testCase.params, testCase.mutated);

		const outcome = await promise;

		expect(outcome.details).toMatchObject({ outcome: "ambiguous", writeState: "possible", retryProhibited: true });
		const posts = pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"));
		expect(posts).toHaveLength(1);
		expect(posts[0]?.[1]).toContain(testCase.endpoint);
		expect(posts[0]?.[2]?.stdin).toBe(JSON.stringify(testCase.payload));
	});

	it.each([
		["Left", "\u001b[D"],
		["Right", "\u001b[C"],
		["Tab", "\t"],
		["Page Up", "\u001b[5~"],
		["Page Down", "\u001b[6~"],
	])("ignores %s for approval selection", async (_label, ignored) => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput(ignored);
			component.handleInput("\r");
			return answer;
		};
		const { tool, pi } = await registered();
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		expect(outcome.details.outcome).toBe("ambiguous");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(1);
	});

	it("rejects lifecycle, session, abort, and changed-origin staleness before mutation", async () => {
		for (const stale of ["lifecycle", "session", "abort", "origin"] as const) {
			const { tool, pi, state, emit } = await registered();
			const controller = new AbortController();
			const custom = async (factory: any) => {
				let answer: unknown;
				const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
					answer = value;
				});
				if (stale === "lifecycle") state.lifecycleGeneration++;
				if (stale === "session") await emit("session_shutdown", {});
				if (stale === "abort") controller.abort();
				if (stale === "origin") pi.exec.mockResolvedValueOnce(execResult("https://github.com/other/repo.git\n"));
				component.handleInput("\r");
				return answer;
			};
			const outcome = await tool.execute(
				"call",
				{ title: "t", body: "b" },
				controller.signal,
				undefined,
				context(custom),
			);
			expect(outcome.details.outcome).toBe("stale");
			const posts = pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"));
			expect(posts).toHaveLength(0);
		}
	});

	it("rechecks freshness after asynchronous origin revalidation", async () => {
		let resolveOrigin!: (value: ReturnType<typeof execResult>) => void;
		const pendingOrigin = new Promise<ReturnType<typeof execResult>>((resolve) => {
			resolveOrigin = resolve;
		});
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput("\r");
			return answer;
		};
		const { tool, pi, state } = await registered();
		pi.exec.mockResolvedValueOnce(execResult("https://github.com/LeanAndMean/scramjet.git\n"));
		pi.exec.mockImplementationOnce(() => pendingOrigin);
		const publication = tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		await vi.waitFor(() => expect(pi.exec).toHaveBeenCalledTimes(2));
		state.lifecycleGeneration++;
		resolveOrigin(execResult("https://github.com/LeanAndMean/scramjet.git\n"));
		const outcome = await publication;
		expect(outcome.details.outcome).toBe("stale");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});
});

describe("owned result hook", () => {
	it("marks only owned failures and ambiguity as errors", async () => {
		const { handlers } = await registered();
		const hook = handlers.get("tool_result")?.[0] as any;
		const owned = {
			toolName: "create_issue",
			details: {
				kind: "scramjet:forge-publication",
				operation: "create_issue",
				outcome: "ambiguous",
				writeState: "possible",
			},
			isError: false,
		};
		expect(await hook(owned)).toEqual({ isError: true });
		expect(
			await hook({ ...owned, details: { ...owned.details, outcome: "cancelled", writeState: "not-dispatched" } }),
		).toEqual({ isError: false });
		expect(await hook({ ...owned, toolName: "other" })).toBeUndefined();
	});
});

function theme() {
	return {
		fg: (_name: string, text: string) => text,
		bg: (_name: string, text: string) => text,
		bold: (text: string) => text,
	};
}
function keybindings() {
	return {
		matches: (data: string, action: string) =>
			({
				"tui.select.confirm": data === "\r",
				"tui.select.cancel": data === "\u001b",
				"tui.select.up": data === "\u001b[A",
				"tui.select.down": data === "\u001b[B",
			})[action] ?? false,
	};
}
