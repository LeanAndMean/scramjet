import { initTheme } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import { projectTerminalSafe, registerForgePublication } from "../src/forge-publication.js";
import { freshState, recordingPi } from "./helpers.js";

initTheme(undefined, false);

function execResult(stdout = "") {
	return { stdout, stderr: "", code: 0, killed: false };
}

function context(custom?: (factory: any) => Promise<any>) {
	return {
		hasUI: Boolean(custom),
		cwd: "/repo",
		ui: custom ? { custom } : undefined,
	};
}

async function registered() {
	const bag = recordingPi();
	bag.pi.exec = vi.fn().mockResolvedValue(execResult("https://github.com/LeanAndMean/scramjet.git\n"));
	const state = freshState();
	registerForgePublication(bag.pi, state);
	return { ...bag, state, tool: bag.tools.find((candidate) => candidate.name === "create_issue") };
}

describe("terminal-safe projection", () => {
	it("is reversible and emits no raw controls, escape sequences, bidi controls, or active Markdown links", () => {
		const input =
			"start\0\t\r\u001b]8;;https://evil.example\u0007link\u001b]8;;\u0007\u202e [x](https://evil.example) end";
		const projected = projectTerminalSafe(input);
		expect(projected.changed).toBe(true);
		expect(projected.text).not.toMatch(/[\u0000-\u001f\u007f-\u009f\u202a-\u202e\u2066-\u2069]/u);
		expect(projected.text).not.toContain("](https://");
		expect(projected.restore()).toBe(input);
	});
});

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

	it.each(["initial Enter", "Escape"])("defaults to Cancel and %s performs no mutation", async (mode) => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput(mode === "initial Enter" ? "\r" : "\u001b");
			return answer;
		};
		const { tool, pi } = await registered();
		const outcome = await tool.execute("call", { title: "t", body: "b" }, undefined, undefined, context(custom));
		expect(outcome.details.outcome).toBe("cancelled");
		expect(pi.exec.mock.calls.filter((call: any[]) => call[1]?.includes("POST"))).toHaveLength(0);
	});

	it("publishes the frozen exact proposal only after explicit approval", async () => {
		const custom = async (factory: any) => {
			let answer: unknown;
			const component = factory({ requestRender() {} }, theme(), keybindings(), (value: unknown) => {
				answer = value;
			});
			component.handleInput("\t");
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
				component.handleInput("\t");
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
			component.handleInput("\t");
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

	it("scrolls complete projected content and never renders raw hostile sequences", async () => {
		let component: any;
		const custom = async (factory: any) => {
			component = factory({ requestRender() {} }, theme(), keybindings(), () => {});
			return "cancelled";
		};
		const { tool } = await registered();
		await tool.execute(
			"call",
			{ title: "BEGIN", body: `${"line\n".repeat(80)}END\u001b]8;;x\u0007` },
			undefined,
			undefined,
			context(custom),
		);
		const first = component.render(70).join("\n");
		expect(first).toContain("BEGIN");
		for (let index = 0; index < 100; index++) component.handleInput("\u001b[6~");
		const last = component.render(70).join("\n");
		expect(last).toContain("END");
		expect(last).not.toContain("\u001b]8;;x");
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
	return { fg: (_name: string, text: string) => text, bold: (text: string) => text };
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
