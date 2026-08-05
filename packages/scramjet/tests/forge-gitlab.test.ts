import { readFileSync } from "node:fs";
import type { ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import { ForgeCommandError, type ForgeExec } from "../src/forge/client.js";
import { createGitlabAdapter } from "../src/forge/gitlab.js";
import type { ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = {
	forge: "gitlab",
	host: "gitlab.com",
	projectPath: "Acme/platform/widget",
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

function ndjson(items: unknown[]): string {
	return items.map((item) => JSON.stringify(item)).join("\n");
}

function listResult(value: unknown): ExecResult {
	if (!Array.isArray(value)) throw new Error("Expected list fixture");
	return result(ndjson(value));
}

interface ReadOverrides {
	issue?: unknown;
	issueNotes?: unknown;
	relationshipPages?: unknown[];
	relationshipResult?: ExecResult;
	pr?: unknown;
	prNotes?: unknown;
	diffs?: unknown;
	commits?: unknown;
	pipelines?: unknown;
}

function readExec(overrides: ReadOverrides = {}): ForgeExec {
	return vi.fn<ForgeExec>(async (_command, args) => {
		if (args[1] === "graphql") {
			return (
				overrides.relationshipResult ??
				result(
					ndjson(overrides.relationshipPages ?? (fixture("gitlab-issue-relationships-pages.json") as unknown[])),
				)
			);
		}
		const endpoint = args.at(-1);
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7") {
			return result(overrides.issue ?? fixture("gitlab-issue.json"));
		}
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7/notes?per_page=100&sort=asc&order_by=created_at") {
			return listResult(overrides.issueNotes ?? fixture("gitlab-issue-notes.json"));
		}
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12") {
			return result(overrides.pr ?? fixture("gitlab-pr.json"));
		}
		if (
			endpoint ===
			"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/notes?per_page=100&sort=asc&order_by=created_at"
		) {
			return listResult(overrides.prNotes ?? fixture("gitlab-pr-notes.json"));
		}
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12/diffs?per_page=100") {
			return listResult(overrides.diffs ?? fixture("gitlab-pr-diffs.json"));
		}
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12/commits?per_page=100") {
			return listResult(overrides.commits ?? fixture("gitlab-pr-commits.json"));
		}
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12/pipelines?per_page=100") {
			return listResult(overrides.pipelines ?? fixture("gitlab-pr-pipelines.json"));
		}
		throw new Error(`Unexpected invocation: ${JSON.stringify(args)}`);
	});
}

function expectReadCall(call: unknown[], args: string[], signal?: AbortSignal): void {
	expect(call).toEqual(["glab", args, { cwd: "/repo", stdin: undefined, timeout: 3000, signal }]);
}

function queryFrom(args: string[]): string {
	const value = args.find((arg) => arg.startsWith("query="));
	if (!value) throw new Error(`Missing GraphQL query in ${JSON.stringify(args)}`);
	return value.slice("query=".length);
}

describe("createGitlabAdapter reads", () => {
	it("normalizes a nested-namespace issue, top-level user notes, and native hierarchy", async () => {
		const controller = new AbortController();
		const exec = readExec();
		const artifact = await createGitlabAdapter(exec, "/repo").readArtifact(
			repository,
			"issue",
			7,
			[],
			controller.signal,
		);

		expect(artifact).toEqual({
			kind: "issue",
			number: 7,
			url: "https://gitlab.com/Acme/platform/widget/-/work_items/7",
			state: "open",
			author: { login: "alice", kind: "user" },
			createdAt: "2026-01-01T00:00:00Z",
			updatedAt: "2026-01-04T00:00:00Z",
			labels: ["bug", "priority:high"],
			assignees: [
				{ login: "bob", kind: "user" },
				{ login: "helper-bot", kind: "bot" },
			],
			title: "Parser <failure>",
			body: "first line\nlast line",
			comments: [
				{
					id: "101",
					url: "https://gitlab.com/Acme/platform/widget/-/work_items/7#note_101",
					author: { login: "helper-bot", kind: "bot" },
					body: "same body",
					createdAt: "2026-01-02T00:00:00Z",
					updatedAt: "2026-01-02T00:30:00Z",
				},
				{
					id: "202",
					url: "https://gitlab.com/Acme/platform/widget/-/work_items/7#note_202",
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
						relation: "parent",
						source: "native",
						number: 3,
						url: "https://gitlab.com/Acme/platform/widget/-/work_items/3",
						state: "open",
						title: "Parent issue",
					},
					{
						relation: "child",
						source: "native",
						number: 8,
						url: "https://gitlab.com/Acme/platform/widget/-/work_items/8",
						state: "closed",
						title: "First child",
					},
					{
						relation: "child",
						source: "native",
						number: 9,
						url: "https://gitlab.com/Acme/platform/widget/-/work_items/9",
						state: "open",
						title: "Second child",
					},
				],
			},
		});

		expect(exec).toHaveBeenCalledTimes(3);
		expectReadCall(
			vi.mocked(exec).mock.calls[0],
			["api", "projects/Acme%2Fplatform%2Fwidget/issues/7"],
			controller.signal,
		);
		expectReadCall(
			vi.mocked(exec).mock.calls[1],
			[
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/issues/7/notes?per_page=100&sort=asc&order_by=created_at",
			],
			controller.signal,
		);
		const relationshipCall = vi.mocked(exec).mock.calls[2];
		const relationshipArgs = relationshipCall[1];
		expect(relationshipArgs.slice(0, 5)).toEqual(["api", "graphql", "--paginate", "--output", "ndjson"]);
		expect(relationshipArgs).toContain("fullPath=Acme/platform/widget");
		expect(relationshipArgs).toContain("iid=7");
		expect(queryFrom(relationshipArgs)).toContain("... on WorkItemWidgetHierarchy");
		expect(queryFrom(relationshipArgs)).toContain("children(first: 100, after: $endCursor)");
		expect(relationshipCall[2]).toEqual({ cwd: "/repo", stdin: undefined, timeout: 3000, signal: controller.signal });
	});

	it("reports hierarchy as unsupported only when a successful response has no hierarchy widget", async () => {
		const relationshipPages = [
			{ data: { project: { workItems: { count: 1, nodes: [{ iid: "7", widgets: [] }] } } } },
		];
		const artifact = await createGitlabAdapter(readExec({ relationshipPages }), "/repo").readArtifact(
			repository,
			"issue",
			7,
			[],
		);
		expect(artifact.kind).toBe("issue");
		if (artifact.kind !== "issue") throw new Error("Expected issue");
		expect(artifact.relationships).toEqual({ capability: "unsupported", items: [] });
	});

	it("does not turn relationship cancellation into unsupported data", async () => {
		const controller = new AbortController();
		controller.abort();
		const exec = readExec({ relationshipResult: result("", { killed: true }) });
		await expect(
			createGitlabAdapter(exec, "/repo").readArtifact(repository, "issue", 7, [], controller.signal),
		).rejects.toMatchObject({ kind: "cancelled" });
		expect(exec).toHaveBeenCalledTimes(3);
	});

	it.each([
		[
			"duplicate child IDs",
			(pages: any[]) => {
				pages[1].data.project.workItems.nodes[0].widgets[0].children.nodes[0].webUrl =
					pages[0].data.project.workItems.nodes[0].widgets[0].children.nodes[0].webUrl;
			},
		],
		[
			"premature pagination",
			(pages: any[]) => {
				pages[1].data.project.workItems.nodes[0].widgets[0].children.pageInfo.hasNextPage = true;
			},
		],
	] as const)("rejects %s instead of returning partial hierarchy data", async (_name, mutate) => {
		const relationshipPages = structuredClone(fixture("gitlab-issue-relationships-pages.json")) as any[];
		mutate(relationshipPages);
		await expect(
			createGitlabAdapter(readExec({ relationshipPages }), "/repo").readArtifact(repository, "issue", 7, []),
		).rejects.toThrow(/GitLab relationships/i);
	});

	it.each([
		[
			"user-note count mismatch",
			() => {
				const issue = structuredClone(fixture("gitlab-issue.json")) as any;
				issue.user_notes_count = 5;
				return { issue };
			},
		],
		[
			"duplicate note IDs",
			() => {
				const issueNotes = structuredClone(fixture("gitlab-issue-notes.json")) as any[];
				issueNotes[1].id = 101;
				return { issueNotes };
			},
		],
	] as const)("rejects %s instead of returning partial conversation data", async (_name, makeOverrides) => {
		await expect(
			createGitlabAdapter(readExec(makeOverrides()), "/repo").readArtifact(repository, "issue", 7, []),
		).rejects.toThrow(/GitLab notes/i);
	});

	it("accepts complete paginated REST output beyond one page", async () => {
		const issue = structuredClone(fixture("gitlab-issue.json")) as any;
		const template = (fixture("gitlab-issue-notes.json") as any[])[0];
		const issueNotes = Array.from({ length: 101 }, (_, index) => ({
			...structuredClone(template),
			id: index + 1,
			body: `comment ${index + 1}`,
		}));
		issue.user_notes_count = issueNotes.length;
		const artifact = await createGitlabAdapter(readExec({ issue, issueNotes }), "/repo").readArtifact(
			repository,
			"issue",
			7,
			[],
		);
		expect(artifact.comments).toHaveLength(101);
	});

	it("normalizes MR readiness and only requested complete optional facets", async () => {
		const exec = readExec();
		const artifact = await createGitlabAdapter(exec, "/repo").readArtifact(repository, "pr", 12, [
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
				reviewDecision: null,
				head: "feature/ship",
				base: "main",
			},
			sections: {
				files: [
					{ path: "src/new.ts", status: "renamed", additions: 5, deletions: 2, previousPath: "src/old.ts" },
					{ path: "README.md", status: "modified", additions: 1, deletions: 0, previousPath: null },
					{ path: "src/add.ts", status: "added", additions: 2, deletions: 0, previousPath: null },
					{ path: "src/delete.ts", status: "deleted", additions: 0, deletions: 1, previousPath: null },
				],
				commits: [
					{
						sha: "abc123",
						title: "First commit",
						author: "Alice",
						createdAt: "2026-02-01T00:00:00Z",
						url: "https://gitlab.com/Acme/platform/widget/-/commit/abc123",
					},
					{
						sha: "def456",
						title: "Second commit",
						author: null,
						createdAt: "2026-02-01T01:00:00Z",
						url: null,
					},
				],
				checks: [
					{
						id: "801",
						name: "test",
						status: "running",
						conclusion: null,
						url: "https://gitlab.com/Acme/platform/widget/-/pipelines/801",
					},
					{
						id: "802",
						name: "pipeline #802",
						status: "success",
						conclusion: "success",
						url: "https://gitlab.com/Acme/platform/widget/-/pipelines/802",
					},
				],
			},
		});
		expect(exec).toHaveBeenCalledTimes(5);
		for (const call of vi.mocked(exec).mock.calls) {
			expect(call[1].join(" ")).toContain("Acme%2Fplatform%2Fwidget");
		}
	});

	it("does not fetch or expose unrequested MR facets", async () => {
		const exec = readExec();
		const artifact = await createGitlabAdapter(exec, "/repo").readArtifact(repository, "pr", 12, []);
		expect(artifact.kind).toBe("pr");
		if (artifact.kind !== "pr") throw new Error("Expected PR");
		expect(artifact.sections).toEqual({});
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it.each([null, "", "1000+"])(
		"allows incomplete diff metadata in core reads but rejects a requested files section (%s)",
		async (changesCount) => {
			const pr = structuredClone(fixture("gitlab-pr.json")) as any;
			pr.changes_count = changesCount;
			await expect(
				createGitlabAdapter(readExec({ pr }), "/repo").readArtifact(repository, "pr", 12, []),
			).resolves.toMatchObject({
				kind: "pr",
				sections: {},
			});
			const exec = readExec({ pr });
			await expect(createGitlabAdapter(exec, "/repo").readArtifact(repository, "pr", 12, ["files"])).rejects.toThrow(
				/GitLab files/i,
			);
			expect(exec).toHaveBeenCalledTimes(2);
		},
	);

	it.each([
		[
			"files",
			() => {
				const diffs = structuredClone(fixture("gitlab-pr-diffs.json")) as any[];
				diffs[0].collapsed = true;
				return { diffs };
			},
		],
		[
			"commits",
			() => {
				const commits = structuredClone(fixture("gitlab-pr-commits.json")) as any[];
				commits[1].id = commits[0].id;
				return { commits };
			},
		],
		[
			"checks",
			() => {
				const pipelines = structuredClone(fixture("gitlab-pr-pipelines.json")) as any[];
				pipelines[1].id = pipelines[0].id;
				return { pipelines };
			},
		],
	] as const)("rejects malformed or incomplete requested %s", async (section, makeOverrides) => {
		await expect(
			createGitlabAdapter(readExec(makeOverrides()), "/repo").readArtifact(repository, "pr", 12, [section]),
		).rejects.toThrow(new RegExp(`GitLab ${section}`, "i"));
	});

	it("rejects a non-GitLab repository before invoking the CLI", async () => {
		const exec = readExec();
		await expect(
			createGitlabAdapter(exec, "/repo").readArtifact(
				{ forge: "github", host: "github.com", projectPath: "Acme/widget" },
				"issue",
				7,
				[],
			),
		).rejects.toThrow(/GitLab repository/);
		expect(exec).not.toHaveBeenCalled();
	});
});

describe("createGitlabAdapter mutations", () => {
	it("uses one explicit-project request with exact JSON stdin and returns response identity", async () => {
		const mutations = fixture("gitlab-mutations.json") as Record<string, unknown>;
		const exec = vi.fn<ForgeExec>(async (_command, args) => {
			const endpoint = args.at(-1);
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues") return result(mutations.issue);
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests") return result(mutations.pr);
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7") {
				return result({
					iid: 7,
					project_id: 9001,
					web_url: "https://gitlab.com/Acme/platform/widget/-/work_items/7",
				});
			}
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12") {
				return result({
					iid: 12,
					project_id: 9001,
					web_url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12",
				});
			}
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7/notes") return result(mutations.issueComment);
			if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12/notes/1234567890") {
				return result(mutations.prComment);
			}
			throw new Error(`Unexpected endpoint ${endpoint}`);
		});
		const adapter = createGitlabAdapter(exec, "/repo");
		const controller = new AbortController();

		await expect(
			adapter.createArtifact(
				repository,
				{ kind: "issue", title: "Created issue", body: "Issue body" },
				controller.signal,
			),
		).resolves.toEqual({
			kind: "issue",
			number: 41,
			url: "https://gitlab.com/Acme/platform/widget/-/work_items/41",
		});
		await expect(
			adapter.createArtifact(repository, {
				kind: "pr",
				title: "Created MR",
				body: "MR body",
				head: "feature",
				base: "main",
				draft: true,
			}),
		).resolves.toEqual({
			kind: "pr",
			number: 42,
			url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/42",
		});
		await expect(
			adapter.updateArtifact(repository, { kind: "issue", number: 7, title: "New title", body: "Body λ\u0000" }),
		).resolves.toEqual({
			kind: "issue",
			number: 7,
			url: "https://gitlab.com/Acme/platform/widget/-/work_items/7",
		});
		await expect(adapter.updateArtifact(repository, { kind: "pr", number: 12, body: "Updated MR" })).resolves.toEqual(
			{ kind: "pr", number: 12, url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12" },
		);
		await expect(adapter.addComment(repository, { kind: "issue", number: 7, body: "Comment body" })).resolves.toEqual(
			{
				kind: "comment",
				id: "9876543210",
				url: "https://gitlab.com/Acme/platform/widget/-/work_items/7#note_9876543210",
			},
		);
		await expect(
			adapter.updateComment(repository, { kind: "pr", number: 12, id: "1234567890", body: "Changed comment" }),
		).resolves.toEqual({
			kind: "comment",
			id: "1234567890",
			url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12#note_1234567890",
		});

		expect(exec).toHaveBeenNthCalledWith(
			1,
			"glab",
			["api", "--method", "POST", "--input", "-", "projects/Acme%2Fplatform%2Fwidget/issues"],
			{
				cwd: "/repo",
				stdin: '{"title":"Created issue","description":"Issue body"}',
				timeout: 3000,
				signal: controller.signal,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			2,
			"glab",
			["api", "--method", "POST", "--input", "-", "projects/Acme%2Fplatform%2Fwidget/merge_requests"],
			{
				cwd: "/repo",
				stdin: '{"title":"Draft: Created MR","description":"MR body","source_branch":"feature","target_branch":"main"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			3,
			"glab",
			["api", "--method", "PUT", "--input", "-", "projects/Acme%2Fplatform%2Fwidget/issues/7"],
			{
				cwd: "/repo",
				stdin: '{"title":"New title","description":"Body λ\\u0000"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			4,
			"glab",
			["api", "--method", "PUT", "--input", "-", "projects/Acme%2Fplatform%2Fwidget/merge_requests/12"],
			{
				cwd: "/repo",
				stdin: '{"description":"Updated MR"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			5,
			"glab",
			["api", "--method", "POST", "--input", "-", "projects/Acme%2Fplatform%2Fwidget/issues/7/notes"],
			{
				cwd: "/repo",
				stdin: '{"body":"Comment body"}',
				timeout: 3000,
				signal: undefined,
			},
		);
		expect(exec).toHaveBeenNthCalledWith(
			6,
			"glab",
			[
				"api",
				"--method",
				"PUT",
				"--input",
				"-",
				"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/notes/1234567890",
			],
			{
				cwd: "/repo",
				stdin: '{"body":"Changed comment"}',
				timeout: 3000,
				signal: undefined,
			},
		);
	});

	it("rejects a draft-prefixed title when draft creation is disabled", async () => {
		const exec = vi.fn<ForgeExec>();
		await expect(
			createGitlabAdapter(exec, "/repo").createArtifact(repository, {
				kind: "pr",
				title: "Draft: Release",
				body: "Body",
				head: "feature",
				base: "main",
				draft: false,
			}),
		).rejects.toThrow(/draft-prefixed title/i);
		expect(exec).not.toHaveBeenCalled();
	});

	it("rejects malformed, mismatched, and unsafe mutation identities", async () => {
		const mutations = fixture("gitlab-mutations.json") as Record<string, unknown>;
		const exec = vi.fn<ForgeExec>(async (_command, args) =>
			args.at(-1) === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12/notes/9876543210"
				? result(mutations.issueComment)
				: result({ iid: 1 }),
		);
		const adapter = createGitlabAdapter(exec, "/repo");
		await expect(adapter.createArtifact(repository, { kind: "issue", title: "Title", body: "Body" })).rejects.toThrow(
			/GitLab mutation response/,
		);
		await expect(
			adapter.updateComment(repository, { kind: "pr", number: 12, id: "9876543210", body: "Body" }),
		).rejects.toThrow(/GitLab mutation response/);
		await expect(
			adapter.updateComment(repository, { kind: "issue", number: 1, id: "../merge_requests/1", body: "Body" }),
		).rejects.toThrow(/note ID/);
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it("preserves classified CLI failures", async () => {
		const exec = vi.fn<ForgeExec>(async () => result("", { spawnError: { code: "ENOENT", message: "missing" } }));
		await expect(
			createGitlabAdapter(exec, "/repo").createArtifact(repository, { kind: "issue", title: "Title", body: "Body" }),
		).rejects.toBeInstanceOf(ForgeCommandError);
	});
});
