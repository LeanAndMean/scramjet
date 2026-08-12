import { describe, expect, it, vi } from "vitest";
import {
	FORGE_EXEC_TIMEOUT_MS,
	type ForgeExec,
	parseForgeOrigin,
	publishForge,
} from "../src/forge-publication-provider.js";

const github = { provider: "github" as const, owner: "LeanAndMean", repository: "scramjet" };

function result(overrides: Record<string, unknown> = {}) {
	return { stdout: "", stderr: "", code: 0, killed: false, ...overrides };
}

describe("parseForgeOrigin", () => {
	it.each([
		["https://github.com/LeanAndMean/scramjet.git", github],
		["git@github.com:LeanAndMean/scramjet.git", github],
		["ssh://git@github.com/LeanAndMean/scramjet.git", github],
		[
			"https://gitlab.com/group/sub/project.git",
			{ provider: "gitlab", namespace: "group/sub", repository: "project" },
		],
	])("accepts canonical public origins", (input, expected) => {
		expect(parseForgeOrigin(input)).toEqual(expected);
	});

	it.each([
		"https://user:pass@github.com/a/b",
		"https://github.com:443/a/b",
		"https://github.com/a/b?x=1",
		"https://github.com/a/b/c",
		"https://github.com/a/%62",
		"git@evil.example:a/b.git",
		"file:///tmp/repo",
		"https://gitlab.com/a/../b",
	])("rejects noncanonical or hostile origins", (input) => {
		expect(() => parseForgeOrigin(input)).toThrow();
	});
});

describe("publishGithubIssue", () => {
	it("posts exact JSON once, refetches the returned number, and verifies exact fields", async () => {
		const proposal = { title: "Unicode café", body: "line 1\r\nline 2\0" };
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(
				result({
					stdout: JSON.stringify({ number: 479, html_url: "https://github.com/LeanAndMean/scramjet/issues/479" }),
				}),
			)
			.mockResolvedValueOnce(
				result({
					stdout: JSON.stringify({
						number: 479,
						title: proposal.title,
						body: proposal.body,
						html_url: "https://github.com/LeanAndMean/scramjet/issues/479",
						pull_request: undefined,
					}),
				}),
			);

		const outcome = await publishForge(exec, github, { operation: "create_issue", ...proposal }, "/repo");

		expect(outcome).toEqual({ status: "verified", url: "https://github.com/LeanAndMean/scramjet/issues/479" });
		expect(exec).toHaveBeenCalledTimes(2);
		expect(exec.mock.calls[0]?.slice(0, 2)).toEqual([
			"gh",
			["api", "--method", "POST", "--input", "-", "repos/LeanAndMean/scramjet/issues"],
		]);
		expect(exec.mock.calls[0]?.[2]).toMatchObject({ cwd: "/repo", stdin: JSON.stringify(proposal) });
		expect(exec.mock.calls[0]?.[2]?.stdin).not.toMatch(/\n$/);
		expect(exec.mock.calls[1]?.[1]).toEqual(["api", "repos/LeanAndMean/scramjet/issues/479"]);
		expect(JSON.stringify(exec.mock.calls[0]?.[1])).not.toContain(proposal.body);
	});

	it("accepts GitHub canonical response casing for a case-insensitive repository identity", async () => {
		const proposal = { title: "title", body: "body" };
		const lower = { provider: "github" as const, owner: "leanandmean", repository: "scramjet" };
		const url = "https://github.com/LeanAndMean/scramjet/issues/7";
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify({ number: 7, html_url: url }) }))
			.mockResolvedValueOnce(
				result({ stdout: JSON.stringify({ number: 7, title: "title", body: "body", html_url: url }) }),
			);
		expect(await publishForge(exec, lower, { operation: "create_issue", ...proposal }, "/repo")).toEqual({
			status: "verified",
			url,
		});
	});

	it("classifies pre-spawn failure as definite and every post-spawn uncertainty as ambiguous without retry", async () => {
		const proposal = { title: "secret title", body: "secret body" };
		const spawnExec = vi
			.fn<ForgeExec>()
			.mockResolvedValue(result({ code: null, spawnError: { message: "ENOENT", code: "ENOENT" } }));
		expect(await publishForge(spawnExec, github, { operation: "create_issue", ...proposal }, "/repo")).toMatchObject({
			status: "no-write",
		});

		const rejectedExec = vi.fn<ForgeExec>().mockRejectedValue(new Error("runtime inactive"));
		expect(await publishForge(rejectedExec, github, { operation: "create_issue", ...proposal }, "/repo")).toEqual({
			status: "ambiguous",
			reason: "mutation-execution-rejected",
			retryProhibited: true,
		});
		expect(rejectedExec).toHaveBeenCalledTimes(1);

		for (const mutation of [
			result({ code: 1, stderr: proposal.body }),
			result({ stdinError: { message: "closed" } }),
			result({ stdout: "not json" }),
			result({ stdout: JSON.stringify({ number: 479, html_url: "https://evil.example/479" }) }),
		]) {
			const exec = vi.fn<ForgeExec>().mockResolvedValue(mutation);
			const outcome = await publishForge(exec, github, { operation: "create_issue", ...proposal }, "/repo");
			expect(outcome).toMatchObject({ status: "ambiguous", retryProhibited: true });
			expect(JSON.stringify(outcome)).not.toContain(proposal.body);
			expect(exec).toHaveBeenCalledTimes(1);
		}
	});

	it("rejects a pull request returned by issue verification", async () => {
		const url = "https://github.com/LeanAndMean/scramjet/issues/8";
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify({ number: 8, html_url: url }) }))
			.mockResolvedValueOnce(
				result({
					stdout: JSON.stringify({
						number: 8,
						title: "title",
						body: "body",
						html_url: url,
						pull_request: {},
					}),
				}),
			);

		expect(
			await publishForge(exec, github, { operation: "create_issue", title: "title", body: "body" }, "/repo"),
		).toMatchObject({ status: "ambiguous", reason: "verification-mismatch", retryProhibited: true });
		expect(exec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);
	});

	it("treats a rejected verification after mutation as ambiguous without retry", async () => {
		const url = "https://github.com/LeanAndMean/scramjet/issues/8";
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify({ number: 8, html_url: url }) }))
			.mockRejectedValueOnce(new Error("runtime invalidated"));

		expect(
			await publishForge(exec, github, { operation: "create_issue", title: "title", body: "body" }, "/repo"),
		).toEqual({ status: "ambiguous", reason: "verification-request-failed", retryProhibited: true });
		expect(exec).toHaveBeenCalledTimes(2);
		expect(exec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);
	});

	it.each(["title", "body"] as const)("treats an exact %s mismatch as ambiguous", async (field) => {
		const proposal = { title: "title", body: "body" };
		const fetched = {
			number: 8,
			title: proposal.title,
			body: proposal.body,
			html_url: "https://github.com/LeanAndMean/scramjet/issues/8",
		};
		fetched[field] += " changed";
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify({ number: 8, html_url: fetched.html_url }) }))
			.mockResolvedValueOnce(result({ stdout: JSON.stringify(fetched) }));
		expect(await publishForge(exec, github, { operation: "create_issue", ...proposal }, "/repo")).toMatchObject({
			status: "ambiguous",
		});
		expect(exec).toHaveBeenCalledTimes(2);
	});
});

// glab v1.112.0 API contract: --input - sends one direct request body without command-level retry.
describe("four-operation provider matrix", () => {
	const gitlab = { provider: "gitlab" as const, namespace: "group/sub", repository: "project" };
	const cases = [
		{
			name: "GitHub PR",
			repo: github,
			request: {
				operation: "create_pr" as const,
				title: "PR",
				body: "body",
				head: "feature",
				base: "main",
				draft: true,
			},
			direct: { number: 12, html_url: "https://github.com/LeanAndMean/scramjet/pull/12" },
			fetched: {
				number: 12,
				html_url: "https://github.com/LeanAndMean/scramjet/pull/12",
				title: "PR",
				body: "body",
				head: { ref: "feature", repo: { full_name: "LeanAndMean/scramjet" } },
				base: { ref: "main", repo: { full_name: "LeanAndMean/scramjet" } },
				draft: true,
			},
			post: "repos/LeanAndMean/scramjet/pulls",
			get: "repos/LeanAndMean/scramjet/pulls/12",
			payload: { title: "PR", body: "body", head: "feature", base: "main", draft: true },
		},
		...(["add_issue_comment", "add_pr_comment"] as const).map((operation, index) => ({
			name: `GitHub ${operation}`,
			repo: github,
			request: { operation, number: 4, body: "comment" },
			direct: {
				id: 90 + index,
				html_url: `https://github.com/LeanAndMean/scramjet/${operation === "add_pr_comment" ? "pull" : "issues"}/4#issuecomment-${90 + index}`,
			},
			fetched: {
				id: 90 + index,
				html_url: `https://github.com/LeanAndMean/scramjet/${operation === "add_pr_comment" ? "pull" : "issues"}/4#issuecomment-${90 + index}`,
				body: "comment",
				issue_url: "https://api.github.com/repos/LeanAndMean/scramjet/issues/4",
			},
			post: "repos/LeanAndMean/scramjet/issues/4/comments",
			get: `repos/LeanAndMean/scramjet/issues/comments/${90 + index}`,
			payload: { body: "comment" },
		})),
		{
			name: "GitLab issue",
			repo: gitlab,
			request: { operation: "create_issue" as const, title: "Issue", body: "body" },
			direct: { iid: 5, web_url: "https://gitlab.com/group/sub/project/-/issues/5" },
			fetched: {
				iid: 5,
				web_url: "https://gitlab.com/group/sub/project/-/issues/5",
				title: "Issue",
				description: "body",
			},
			post: "projects/group%2Fsub%2Fproject/issues",
			get: "projects/group%2Fsub%2Fproject/issues/5",
			payload: { title: "Issue", description: "body" },
		},
		{
			name: "GitLab MR",
			repo: gitlab,
			request: {
				operation: "create_pr" as const,
				title: "Draft: PR",
				body: "body",
				head: "feature",
				base: "main",
				draft: true,
			},
			direct: { iid: 6, web_url: "https://gitlab.com/group/sub/project/-/merge_requests/6" },
			fetched: {
				iid: 6,
				web_url: "https://gitlab.com/group/sub/project/-/merge_requests/6",
				title: "Draft: PR",
				description: "body",
				source_branch: "feature",
				target_branch: "main",
				project_id: 42,
				source_project_id: 42,
				target_project_id: 42,
				draft: true,
			},
			post: "projects/group%2Fsub%2Fproject/merge_requests",
			get: "projects/group%2Fsub%2Fproject/merge_requests/6",
			payload: { title: "Draft: PR", description: "body", source_branch: "feature", target_branch: "main" },
		},
		...(["add_issue_comment", "add_pr_comment"] as const).map((operation, index) => ({
			name: `GitLab ${operation}`,
			repo: gitlab,
			request: { operation, number: 7, body: "comment" },
			direct: {
				id: 70 + index,
				project_id: 42,
				noteable_iid: 7,
				noteable_type: operation === "add_pr_comment" ? "MergeRequest" : "Issue",
			},
			fetched: {
				id: 70 + index,
				project_id: 42,
				body: "comment",
				noteable_iid: 7,
				noteable_type: operation === "add_pr_comment" ? "MergeRequest" : "Issue",
			},
			post: `projects/group%2Fsub%2Fproject/${operation === "add_pr_comment" ? "merge_requests" : "issues"}/7/notes`,
			get: `projects/group%2Fsub%2Fproject/${operation === "add_pr_comment" ? "merge_requests" : "issues"}/7/notes/${70 + index}`,
			payload: { body: "comment" },
		})),
	];
	it.each(cases)(
		"maps and exactly verifies $name with one POST",
		async ({ repo, request, direct, fetched, post, get, payload }) => {
			const exec = vi
				.fn<ForgeExec>()
				.mockResolvedValueOnce(result({ stdout: JSON.stringify(direct) }))
				.mockResolvedValueOnce(result({ stdout: JSON.stringify(fetched) }));
			const outcome = await publishForge(exec, repo, request, "/repo");
			expect(outcome.status).toBe("verified");
			expect(exec).toHaveBeenCalledTimes(2);
			expect(exec.mock.calls[0]?.[1]).toEqual(["api", "--method", "POST", "--input", "-", post]);
			expect(exec.mock.calls[0]?.[2]).toMatchObject({
				stdin: JSON.stringify(payload),
				timeout: FORGE_EXEC_TIMEOUT_MS,
			});
			expect(exec.mock.calls[1]?.[1]).toEqual(["api", get]);
			expect(exec.mock.calls[1]?.[2]).toMatchObject({ timeout: FORGE_EXEC_TIMEOUT_MS });
			expect(exec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);
		},
	);

	it.each([
		[
			"GitHub PR head branch",
			0,
			(value: any) => {
				value.head.ref = "other";
			},
		],
		[
			"GitHub PR base branch",
			0,
			(value: any) => {
				value.base.ref = "other";
			},
		],
		[
			"GitHub PR head repository",
			0,
			(value: any) => {
				value.head.repo.full_name = "other/repo";
			},
		],
		[
			"GitHub PR base repository",
			0,
			(value: any) => {
				value.base.repo.full_name = "other/repo";
			},
		],
		[
			"GitHub PR draft state",
			0,
			(value: any) => {
				value.draft = false;
			},
		],
		[
			"GitHub issue-comment parent",
			1,
			(value: any) => {
				value.issue_url = value.issue_url.replace("/4", "/5");
			},
		],
		[
			"GitHub PR-comment parent",
			2,
			(value: any) => {
				value.issue_url = value.issue_url.replace("/4", "/5");
			},
		],
		[
			"GitLab MR source branch",
			4,
			(value: any) => {
				value.source_branch = "other";
			},
		],
		[
			"GitLab MR target branch",
			4,
			(value: any) => {
				value.target_branch = "other";
			},
		],
		[
			"GitLab MR source project",
			4,
			(value: any) => {
				value.source_project_id = 99;
			},
		],
		[
			"GitLab MR target project",
			4,
			(value: any) => {
				value.target_project_id = 99;
			},
		],
		[
			"GitLab MR draft state",
			4,
			(value: any) => {
				value.draft = false;
			},
		],
		[
			"GitLab issue-note parent",
			5,
			(value: any) => {
				value.noteable_iid = 8;
			},
		],
		[
			"GitLab issue-note type",
			5,
			(value: any) => {
				value.noteable_type = "MergeRequest";
			},
		],
		[
			"GitLab issue-note project",
			5,
			(value: any) => {
				value.project_id = 99;
			},
		],
		[
			"GitLab MR-note parent",
			6,
			(value: any) => {
				value.noteable_iid = 8;
			},
		],
		[
			"GitLab MR-note type",
			6,
			(value: any) => {
				value.noteable_type = "Issue";
			},
		],
		[
			"GitLab MR-note project",
			6,
			(value: any) => {
				value.project_id = 99;
			},
		],
	] as const)("rejects an operation-specific %s mismatch without retry", async (_name, caseIndex, mutate) => {
		const testCase = cases[caseIndex]!;
		const fetched = structuredClone(testCase.fetched) as any;
		mutate(fetched);
		const exec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify(testCase.direct) }))
			.mockResolvedValueOnce(result({ stdout: JSON.stringify(fetched) }));

		expect(await publishForge(exec, testCase.repo, testCase.request, "/repo")).toMatchObject({
			status: "ambiguous",
			reason: "verification-mismatch",
			retryProhibited: true,
		});
		expect(exec).toHaveBeenCalledTimes(2);
		expect(exec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);
	});

	it("rejects cross-repository PR identity and GitLab note project mismatches without retry", async () => {
		const githubUrl = "https://github.com/LeanAndMean/scramjet/pull/12";
		const githubExec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(result({ stdout: JSON.stringify({ number: 12, html_url: githubUrl }) }))
			.mockResolvedValueOnce(
				result({
					stdout: JSON.stringify({
						number: 12,
						html_url: githubUrl,
						title: "PR",
						body: "body",
						head: { ref: "feature", repo: { full_name: "other/repo" } },
						base: { ref: "main", repo: { full_name: "LeanAndMean/scramjet" } },
						draft: false,
					}),
				}),
			);
		expect(
			await publishForge(
				githubExec,
				github,
				{ operation: "create_pr", title: "PR", body: "body", head: "feature", base: "main", draft: false },
				"/repo",
			),
		).toMatchObject({ status: "ambiguous", retryProhibited: true });
		expect(githubExec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);

		const gitlabExec = vi
			.fn<ForgeExec>()
			.mockResolvedValueOnce(
				result({ stdout: JSON.stringify({ id: 70, project_id: 42, noteable_iid: 7, noteable_type: "Issue" }) }),
			)
			.mockResolvedValueOnce(
				result({
					stdout: JSON.stringify({
						id: 70,
						project_id: 99,
						noteable_iid: 7,
						noteable_type: "Issue",
						body: "comment",
					}),
				}),
			);
		expect(
			await publishForge(
				gitlabExec,
				gitlab,
				{ operation: "add_issue_comment", number: 7, body: "comment" },
				"/repo",
			),
		).toMatchObject({ status: "ambiguous", retryProhibited: true });
		expect(gitlabExec.mock.calls.filter((call) => call[1].includes("POST"))).toHaveLength(1);
	});
});
