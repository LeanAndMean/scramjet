import { describe, expect, it, vi } from "vitest";
import { type ForgeExec, parseForgeOrigin, publishGithubIssue } from "../src/forge-publication-provider.js";

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

		const outcome = await publishGithubIssue(exec, github, proposal, "/repo");

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
		expect(await publishGithubIssue(exec, lower, proposal, "/repo")).toEqual({ status: "verified", url });
	});

	it("classifies pre-spawn failure as definite and every post-spawn uncertainty as ambiguous without retry", async () => {
		const proposal = { title: "secret title", body: "secret body" };
		const spawnExec = vi
			.fn<ForgeExec>()
			.mockResolvedValue(result({ code: null, spawnError: { message: "ENOENT", code: "ENOENT" } }));
		expect(await publishGithubIssue(spawnExec, github, proposal, "/repo")).toMatchObject({ status: "no-write" });

		for (const mutation of [
			result({ code: 1, stderr: proposal.body }),
			result({ stdinError: { message: "closed" } }),
			result({ stdout: "not json" }),
			result({ stdout: JSON.stringify({ number: 479, html_url: "https://evil.example/479" }) }),
		]) {
			const exec = vi.fn<ForgeExec>().mockResolvedValue(mutation);
			const outcome = await publishGithubIssue(exec, github, proposal, "/repo");
			expect(outcome).toMatchObject({ status: "ambiguous", retryProhibited: true });
			expect(JSON.stringify(outcome)).not.toContain(proposal.body);
			expect(exec).toHaveBeenCalledTimes(1);
		}
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
		expect(await publishGithubIssue(exec, github, proposal, "/repo")).toMatchObject({ status: "ambiguous" });
		expect(exec).toHaveBeenCalledTimes(2);
	});
});
