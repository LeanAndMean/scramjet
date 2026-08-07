import { readFileSync } from "node:fs";
import type { ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import { ForgeCommandError, type ForgeExec } from "../src/forge/client.js";
import { createGithubAdapter } from "../src/forge/github.js";
import type { ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = {
	forge: "github",
	host: "github.com",
	projectPath: "Acme/widget",
};

function fixture(name: string): unknown {
	return JSON.parse(readFileSync(new URL(`fixtures/forge/${name}`, import.meta.url), "utf8"));
}

function result(stdout: unknown, overrides: Partial<ExecResult> = {}): ExecResult {
	return {
		stdout: typeof stdout === "string" ? stdout : JSON.stringify(stdout),
		stderr: "",
		code: 0,
		killed: false,
		...overrides,
	};
}

function queryFrom(args: string[]): string {
	const query = args.find((arg) => arg.startsWith("query="));
	if (!query) throw new Error(`Missing GraphQL query in ${JSON.stringify(args)}`);
	return query.slice("query=".length);
}

function readExec(overrides: { relationships?: ExecResult; issuePages?: unknown; prPages?: unknown } = {}): ForgeExec {
	return vi.fn<ForgeExec>(async (_command, args) => {
		if (args[1] === "graphql") {
			const query = queryFrom(args);
			if (query.includes("subIssues(")) {
				return overrides.relationships ?? result(fixture("github-issue-relationships-pages.json"));
			}
			if (query.includes("commits(")) return result(fixture("github-pr-commits-pages.json"));
			if (query.includes("statusCheckRollup")) return result(fixture("github-pr-checks-pages.json"));
			if (query.includes("TaskListIssue")) {
				return result({
					data: {
						repository: {
							task: {
								number: 8,
								url: "https://github.com/Acme/widget/issues/8",
								state: "OPEN",
								title: "Task-list child",
							},
						},
					},
				});
			}
			if (query.includes("issue(number:")) {
				return result(overrides.issuePages ?? fixture("github-issue-pages.json"));
			}
			if (query.includes("pullRequest(number:")) {
				return result(overrides.prPages ?? fixture("github-pr-pages.json"));
			}
		}
		if (args.includes("repos/Acme/widget/pulls/12/files?per_page=100")) {
			return result(fixture("github-pr-files-pages.json"));
		}
		throw new Error(`Unexpected invocation: ${JSON.stringify(args)}`);
	});
}

function expectGraphqlInvocation(call: unknown[], number: number, signal?: AbortSignal): string {
	const [command, args, options] = call as [string, string[], Record<string, unknown>];
	expect(command).toBe("gh");
	expect(args.slice(0, 4)).toEqual(["api", "graphql", "--paginate", "--slurp"]);
	expect(args.slice(6)).toEqual(["-f", "owner=Acme", "-f", "name=widget", "-F", `number=${number}`]);
	expect(options).toEqual({ cwd: "/repo", stdin: undefined, timeout: 3000, signal });
	return queryFrom(args);
}

describe("createGithubAdapter reads", () => {
	it("normalizes a complete multi-page issue with native relationships", async () => {
		const controller = new AbortController();
		const exec = readExec();
		const adapter = createGithubAdapter(exec, "/repo");

		await expect(adapter.readArtifact(repository, "issue", 7, [], controller.signal)).resolves.toEqual({
			kind: "issue",
			number: 7,
			url: "https://github.com/Acme/widget/issues/7",
			state: "open",
			author: { login: "alice", kind: "user" },
			createdAt: "2026-01-01T00:00:00Z",
			updatedAt: "2026-01-04T00:00:00Z",
			labels: ["bug", "priority:high"],
			assignees: [
				{ login: "bob", kind: "user" },
				{ login: "helper[bot]", kind: "bot" },
			],
			title: "Parser <failure>",
			body: "first line\n- [ ] #8\n- [ ] Other/repository#10\n- [x] Related to #99 and #100\nlast line",
			comments: [
				{
					id: "101",
					url: "https://github.com/Acme/widget/issues/7#issuecomment-101",
					author: { login: "helper[bot]", kind: "bot" },
					body: "same body",
					createdAt: "2026-01-02T00:00:00Z",
					updatedAt: "2026-01-02T00:30:00Z",
				},
				{
					id: "202",
					url: "https://github.com/Acme/widget/issues/7#issuecomment-202",
					author: { login: null, kind: "deleted" },
					body: "same body",
					createdAt: "2026-01-03T00:00:00Z",
					updatedAt: "2026-01-03T00:00:00Z",
				},
			],
			relationships: {
				capability: "supported",
				items: [
					{
						repository,
						relation: "parent",
						source: "native",
						number: 3,
						url: "https://github.com/Acme/widget/issues/3",
						state: "open",
						title: "Parent issue",
					},
					{
						repository,
						relation: "child",
						source: "native",
						number: 8,
						url: "https://github.com/Acme/widget/issues/8",
						state: "closed",
						title: "First child",
					},
					{
						repository,
						relation: "child",
						source: "native",
						number: 9,
						url: "https://github.com/Acme/widget/issues/9",
						state: "open",
						title: "Second child",
					},
					{
						repository: { ...repository, projectPath: "Other/repository" },
						relation: "child",
						source: "native",
						number: 8,
						url: "https://github.com/Other/repository/issues/8",
						state: "open",
						title: "External same-number child",
					},
				],
			},
		});

		expect(exec).toHaveBeenCalledTimes(2);
		const coreQuery = expectGraphqlInvocation(vi.mocked(exec).mock.calls[0], 7, controller.signal);
		expect(coreQuery).toContain("issue(number: $number)");
		expect(coreQuery).toContain("comments(first: 100, after: $endCursor)");
		const relationshipQuery = expectGraphqlInvocation(vi.mocked(exec).mock.calls[1], 7, controller.signal);
		expect(relationshipQuery).toContain("parent {");
		expect(relationshipQuery).toContain("subIssues(first: 100, after: $endCursor)");
	});

	it("uses the same-repository task-list fallback only when native relationships are unsupported", async () => {
		const exec = readExec({
			relationships: result(
				[
					{
						errors: [
							{
								extensions: { code: "undefinedField", typeName: "Issue", fieldName: "subIssues" },
							},
						],
					},
				],
				{ code: 1, stderr: "Field subIssues does not exist on type Issue" },
			),
		});
		const artifact = await createGithubAdapter(exec, "/repo").readArtifact(repository, "issue", 7, []);

		expect(artifact.kind).toBe("issue");
		if (artifact.kind !== "issue") throw new Error("Expected issue");
		expect(artifact.relationships.items).toEqual([
			{
				repository,
				relation: "child",
				source: "task-list",
				number: 8,
				url: "https://github.com/Acme/widget/issues/8",
				state: "open",
				title: "Task-list child",
			},
		]);
		expect(exec).toHaveBeenCalledTimes(3);
		const fallbackQuery = queryFrom(vi.mocked(exec).mock.calls[2][1]);
		expect(fallbackQuery).toContain("query TaskListIssue");
		expect(vi.mocked(exec).mock.calls[2][1]).toContain("number=8");
		for (const excluded of [10, 99, 100]) {
			expect(vi.mocked(exec).mock.calls[2][1]).not.toContain(`number=${excluded}`);
		}
	});

	it.each([
		result([{ errors: [{ extensions: { code: "RATE_LIMITED" } }] }], { code: 1, stderr: "rate limited" }),
		result("", { code: 1, stderr: "HTTP 401 Unauthorized" }),
		result("not-json", { code: 1, stderr: "network failure" }),
	])("propagates operational native relationship failures without fallback", async (relationships) => {
		const exec = readExec({ relationships });
		await expect(createGithubAdapter(exec, "/repo").readArtifact(repository, "issue", 7, [])).rejects.toBeInstanceOf(
			ForgeCommandError,
		);
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it("does not turn cancellation into task-list fallback data", async () => {
		const controller = new AbortController();
		controller.abort();
		const exec = readExec({ relationships: result("", { killed: true }) });
		await expect(
			createGithubAdapter(exec, "/repo").readArtifact(repository, "issue", 7, [], controller.signal),
		).rejects.toMatchObject({ kind: "cancelled" });
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it("normalizes PR readiness and only requested complete optional facets", async () => {
		const exec = readExec();
		const artifact = await createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, [
			"checks",
			"files",
			"commits",
		]);

		expect(artifact).toMatchObject({
			kind: "pr",
			number: 12,
			state: "open",
			readiness: {
				draft: false,
				mergeable: "mergeable",
				reviewDecision: { capability: "supported", value: "approved" },
				head: "feature/ship",
				base: "main",
			},
			sections: {
				files: [
					{ path: "src/new.ts", status: "renamed", additions: 5, deletions: 2, previousPath: "src/old.ts" },
					{ path: "README.md", status: "modified", additions: 1, deletions: 0, previousPath: null },
				],
				commits: [
					{
						sha: "abc123",
						title: "First commit",
						author: "Alice",
						createdAt: "2026-02-01T00:00:00Z",
						url: "https://github.com/Acme/widget/commit/abc123",
					},
					{
						sha: "def456",
						title: "Second commit",
						author: null,
						createdAt: "2026-02-01T01:00:00Z",
						url: "https://github.com/Acme/widget/commit/def456",
					},
				],
				checks: [
					{
						id: "CR_kwDOAA",
						name: "test",
						status: "in_progress",
						conclusion: null,
						url: "https://github.com/Acme/widget/actions/runs/1",
					},
					{
						id: "SC_kwDOBB",
						name: "lint",
						status: "completed",
						conclusion: "success",
						url: null,
					},
				],
			},
		});
		expect(exec).toHaveBeenCalledTimes(4);
		const filesCall = vi
			.mocked(exec)
			.mock.calls.find((call) => call[1].includes("repos/Acme/widget/pulls/12/files?per_page=100"));
		expect(filesCall).toEqual([
			"gh",
			["api", "--paginate", "--slurp", "repos/Acme/widget/pulls/12/files?per_page=100"],
			{ cwd: "/repo", stdin: undefined, timeout: 3000, signal: undefined },
		]);
	});

	it("keeps unknown pull request readiness values explicitly non-favorable", async () => {
		const pages = structuredClone(fixture("github-pr-pages.json")) as any[];
		pages[0].data.repository.pullRequest.reviewDecision = "FUTURE_DECISION";
		pages[0].data.repository.pullRequest.mergeable = "FUTURE_MERGEABILITY";
		const artifact = await createGithubAdapter(readExec({ prPages: pages }), "/repo").readArtifact(
			repository,
			"pr",
			12,
			[],
		);
		expect(artifact.kind).toBe("pr");
		if (artifact.kind !== "pr") throw new Error("Expected PR");
		expect(artifact.readiness).toMatchObject({
			mergeable: "unknown",
			reviewDecision: { capability: "unknown", value: "future_decision" },
		});
	});

	it("preserves future actors and check states without favorable guesses", async () => {
		const issuePages = structuredClone(fixture("github-issue-pages.json")) as any[];
		for (const page of issuePages) page.data.repository.issue.author.__typename = "FutureActor";
		const unknownActor = await createGithubAdapter(readExec({ issuePages }), "/repo").readArtifact(
			repository,
			"issue",
			7,
			[],
		);
		expect(unknownActor.author).toEqual({ login: "alice", kind: "unknown" });

		const checkPages = structuredClone(fixture("github-pr-checks-pages.json")) as any[];
		checkPages[1].data.repository.pullRequest.statusCheckRollup.contexts.nodes[0].state = "FUTURE_STATE";
		const fallback = readExec();
		const exec = vi.fn<ForgeExec>(async (command, args, options) => {
			if (args[1] === "graphql" && queryFrom(args).includes("statusCheckRollup")) return result(checkPages);
			return fallback(command, args, options);
		});
		const artifact = await createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, ["checks"]);
		expect(artifact.kind).toBe("pr");
		if (artifact.kind !== "pr") throw new Error("Expected PR");
		expect(artifact.sections.checks).toContainEqual({
			id: "SC_kwDOBB",
			name: "lint",
			status: "unknown",
			conclusion: null,
			url: null,
		});
	});

	it("does not fetch or expose unrequested PR facets", async () => {
		const exec = readExec();
		const artifact = await createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, []);
		expect(artifact.kind).toBe("pr");
		if (artifact.kind !== "pr") throw new Error("Expected PR");
		expect(artifact.sections).toEqual({});
		expect(exec).toHaveBeenCalledTimes(1);
	});

	it.each([
		[
			"comment count mismatch",
			(pages: any[]) => {
				pages[0].data.repository.issue.comments.totalCount = 3;
			},
		],
		[
			"duplicate comment IDs",
			(pages: any[]) => {
				pages[1].data.repository.issue.comments.nodes[0].databaseId = 101;
			},
		],
		[
			"premature pagination",
			(pages: any[]) => {
				pages[1].data.repository.issue.comments.pageInfo.hasNextPage = true;
			},
		],
	] as const)("rejects %s instead of returning partial issue data", async (_name, mutate) => {
		const pages = structuredClone(fixture("github-issue-pages.json")) as any[];
		mutate(pages);
		const exec = readExec({ issuePages: pages });
		await expect(createGithubAdapter(exec, "/repo").readArtifact(repository, "issue", 7, [])).rejects.toThrow(
			/GitHub.*comments/i,
		);
	});

	it("rejects incomplete files rather than silently returning a partial section", async () => {
		const exec = readExec();
		vi.mocked(exec).mockImplementation(async (_command, args) => {
			if (args.includes("repos/Acme/widget/pulls/12/files?per_page=100")) {
				const pages = fixture("github-pr-files-pages.json") as unknown[][];
				return result([[pages[0][0]]]);
			}
			if (args[1] === "graphql" && queryFrom(args).includes("pullRequest(number:")) {
				return result(fixture("github-pr-pages.json"));
			}
			throw new Error("unexpected");
		});
		await expect(createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, ["files"])).rejects.toThrow(
			/GitHub files/i,
		);
	});

	it("rejects malformed requested checks rather than silently omitting them", async () => {
		const exec = readExec();
		vi.mocked(exec).mockImplementation(async (_command, args) => {
			if (args[1] === "graphql") {
				const query = queryFrom(args);
				if (query.includes("statusCheckRollup")) return result([{ data: { repository: { pullRequest: null } } }]);
				if (query.includes("pullRequest(number:")) return result(fixture("github-pr-pages.json"));
			}
			throw new Error("unexpected");
		});
		await expect(createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, ["checks"])).rejects.toThrow(
			/GitHub checks/i,
		);
	});

	it.each([
		["commits", "github-pr-commits-pages.json", "commits("],
		["checks", "github-pr-checks-pages.json", "statusCheckRollup"],
	] as const)("rejects %s returned for another PR", async (section, fixtureName, queryToken) => {
		const pages = structuredClone(fixture(fixtureName)) as any[];
		pages[1].data.repository.pullRequest.number = 13;
		const exec = vi.fn<ForgeExec>(async (_command, args) => {
			const query = queryFrom(args);
			if (query.includes(queryToken)) return result(pages);
			if (query.includes("pullRequest(number:")) return result(fixture("github-pr-pages.json"));
			throw new Error("unexpected");
		});
		await expect(createGithubAdapter(exec, "/repo").readArtifact(repository, "pr", 12, [section])).rejects.toThrow(
			new RegExp(`GitHub ${section}`, "i"),
		);
	});

	it("rejects a non-GitHub repository before invoking the CLI", async () => {
		const exec = readExec();
		await expect(
			createGithubAdapter(exec, "/repo").readArtifact(
				{ forge: "gitlab", host: "gitlab.com", projectPath: "Acme/widget" },
				"issue",
				7,
				[],
			),
		).rejects.toThrow(/GitHub repository/);
		expect(exec).not.toHaveBeenCalled();
	});
});

describe("createGithubAdapter mutations", () => {
	it("uses one explicit-repository request with exact JSON stdin and returns response identity", async () => {
		const mutations = fixture("github-mutations.json") as Record<string, unknown>;
		const exec = vi.fn<ForgeExec>(async (_command, args) => {
			const endpoint = args.at(-1);
			if (endpoint === "repos/Acme/widget/issues") return result(mutations.issue);
			if (endpoint === "repos/Acme/widget/pulls") return result(mutations.pr);
			if (endpoint === "repos/Acme/widget/issues/7") {
				return result({ number: 7, html_url: "https://github.com/Acme/widget/issues/7" });
			}
			if (endpoint === "repos/Acme/widget/pulls/12") {
				return result({ number: 12, html_url: "https://github.com/Acme/widget/pull/12" });
			}
			if (endpoint === "repos/Acme/widget/issues/7/comments") return result(mutations.issueComment);
			if (endpoint === "repos/Acme/widget/issues/comments/1234567890") return result(mutations.prComment);
			throw new Error(`Unexpected endpoint ${endpoint}`);
		});
		const adapter = createGithubAdapter(exec, "/repo");
		const controller = new AbortController();

		await expect(
			adapter.createArtifact(
				repository,
				{ kind: "issue", title: "Created issue", body: "Issue body" },
				controller.signal,
			),
		).resolves.toEqual({ kind: "issue", number: 41, url: "https://github.com/Acme/widget/issues/41" });
		await expect(
			adapter.createArtifact(repository, {
				kind: "pr",
				title: "Created PR",
				body: "PR body",
				head: "feature",
				base: "main",
				draft: true,
			}),
		).resolves.toEqual({ kind: "pr", number: 42, url: "https://github.com/Acme/widget/pull/42" });
		await expect(
			adapter.updateArtifact(repository, { kind: "issue", number: 7, title: "New title", body: "Body λ\u0000" }),
		).resolves.toEqual({ kind: "issue", number: 7, url: "https://github.com/Acme/widget/issues/7" });
		await expect(adapter.updateArtifact(repository, { kind: "pr", number: 12, body: "Updated PR" })).resolves.toEqual(
			{ kind: "pr", number: 12, url: "https://github.com/Acme/widget/pull/12" },
		);
		await expect(adapter.addComment(repository, { kind: "issue", number: 7, body: "Comment body" })).resolves.toEqual(
			{
				kind: "comment",
				id: "9876543210",
				url: "https://github.com/Acme/widget/issues/7#issuecomment-9876543210",
			},
		);
		await expect(
			adapter.updateComment(repository, { kind: "pr", number: 12, id: "1234567890", body: "Changed comment" }),
		).resolves.toEqual({
			kind: "comment",
			id: "1234567890",
			url: "https://github.com/Acme/widget/pull/12#issuecomment-1234567890",
		});

		expect(exec).toHaveBeenNthCalledWith(
			1,
			"gh",
			["api", "--method", "POST", "--input", "-", "repos/Acme/widget/issues"],
			{
				cwd: "/repo",
				stdin: '{"title":"Created issue","body":"Issue body"}',
				timeout: 3000,
				signal: controller.signal,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			2,
			"gh",
			["api", "--method", "POST", "--input", "-", "repos/Acme/widget/pulls"],
			{
				cwd: "/repo",
				stdin: '{"title":"Created PR","body":"PR body","head":"feature","base":"main","draft":true}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			3,
			"gh",
			["api", "--method", "PATCH", "--input", "-", "repos/Acme/widget/issues/7"],
			{
				cwd: "/repo",
				stdin: '{"title":"New title","body":"Body λ\\u0000"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			4,
			"gh",
			["api", "--method", "PATCH", "--input", "-", "repos/Acme/widget/pulls/12"],
			{
				cwd: "/repo",
				stdin: '{"body":"Updated PR"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			5,
			"gh",
			["api", "--method", "POST", "--input", "-", "repos/Acme/widget/issues/7/comments"],
			{
				cwd: "/repo",
				stdin: '{"body":"Comment body"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			6,
			"gh",
			["api", "--method", "PATCH", "--input", "-", "repos/Acme/widget/issues/comments/1234567890"],
			{
				cwd: "/repo",
				stdin: '{"body":"Changed comment"}',
				timeout: 3000,
				signal: undefined,
			},
		);
	});

	it("accepts canonical response casing for a lowercase repository origin", async () => {
		const mutations = fixture("github-mutations.json") as Record<string, unknown>;
		const lowercase = { ...repository, projectPath: "acme/widget" };
		const exec = vi.fn<ForgeExec>(async (_command, args) => {
			if (args.at(-1) === "repos/acme/widget/issues") return result(mutations.issue);
			if (args.at(-1) === "repos/acme/widget/issues/7/comments") return result(mutations.issueComment);
			throw new Error(`Unexpected endpoint ${args.at(-1)}`);
		});
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.createArtifact(lowercase, { kind: "issue", title: "Title", body: "Body" })).resolves.toEqual(
			{ kind: "issue", number: 41, url: "https://github.com/Acme/widget/issues/41" },
		);
		await expect(adapter.addComment(lowercase, { kind: "issue", number: 7, body: "Body" })).resolves.toEqual({
			kind: "comment",
			id: "9876543210",
			url: "https://github.com/Acme/widget/issues/7#issuecomment-9876543210",
		});
	});

	it("rejects malformed, mismatched, and unsafe mutation identities", async () => {
		const mutations = fixture("github-mutations.json") as Record<string, unknown>;
		const exec = vi.fn<ForgeExec>(async (_command, args) =>
			args.at(-1) === "repos/Acme/widget/issues/comments/9876543210"
				? result(mutations.issueComment)
				: result({ number: 1 }),
		);
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.createArtifact(repository, { kind: "issue", title: "Title", body: "Body" })).rejects.toThrow(
			/GitHub mutation response/,
		);
		await expect(
			adapter.updateComment(repository, { kind: "pr", number: 12, id: "9876543210", body: "Body" }),
		).rejects.toThrow(/GitHub mutation response/);
		await expect(
			adapter.updateComment(repository, { kind: "issue", number: 1, id: "../pulls/1", body: "Body" }),
		).rejects.toThrow(/comment ID/);
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it("preserves classified CLI failures", async () => {
		const exec = vi.fn<ForgeExec>(async () => result("", { spawnError: { code: "ENOENT", message: "missing" } }));
		await expect(
			createGithubAdapter(exec, "/repo").createArtifact(repository, { kind: "issue", title: "Title", body: "Body" }),
		).rejects.toBeInstanceOf(ForgeCommandError);
	});
});
