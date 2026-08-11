import type { ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import type { ForgeExec } from "../src/forge/client.js";
import { createGithubAdapter } from "../src/forge/github.js";
import type { ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = { forge: "github", host: "github.com", projectPath: "Acme/widget" };

function result(stdout: unknown, overrides: Partial<ExecResult> = {}): ExecResult {
	return {
		stdout: typeof stdout === "string" ? stdout : JSON.stringify(stdout),
		stderr: "",
		code: 0,
		killed: false,
		...overrides,
	};
}

const issue = {
	number: 7,
	html_url: "https://github.com/Acme/widget/issues/7",
	title: "Parser failure",
	body: "first\tline",
};
const pr = {
	number: 12,
	html_url: "https://github.com/Acme/widget/pull/12",
	title: "Ship parser",
	body: "Ready",
	draft: false,
	head: { ref: "feature/parser" },
	base: { ref: "main" },
};
const comment = {
	id: 101,
	html_url: "https://github.com/Acme/widget/issues/7#issuecomment-101",
	issue_url: "https://api.github.com/repos/Acme/widget/issues/7",
	body: "Investigating",
};

function readExec(): ForgeExec {
	return vi.fn(async (_command, args) => {
		const endpoint = args.at(-1);
		if (endpoint === "repos/Acme/widget/issues/7") return result(issue);
		if (endpoint === "repos/Acme/widget/pulls/12") return result(pr);
		if (endpoint === "repos/Acme/widget/issues/comments/101") return result(comment);
		throw new Error(`Unexpected invocation: ${JSON.stringify(args)}`);
	});
}

describe("createGithubAdapter native reads", () => {
	it("pins every GitHub issue command and preserves --slurp page shapes", () => {
		const adapter = createGithubAdapter(readExec(), "/repo");
		const plan = adapter.readPlan(repository, "issue", 7, ["artifact", "comments", "sub_issues", "parent"]);
		expect(
			plan.segments.map(({ id, command, args, shape, optional, evidence }) => ({
				id,
				command,
				args,
				shape,
				optional,
				evidence,
			})),
		).toEqual([
			{
				id: "artifact",
				command: "gh",
				args: ["api", "repos/Acme/widget/issues/7"],
				shape: { kind: "json" },
				optional: undefined,
				evidence: "artifact",
			},
			{
				id: "comments",
				command: "gh",
				args: ["api", "--paginate", "--slurp", "repos/Acme/widget/issues/7/comments?per_page=100"],
				shape: { kind: "gh-slurp" },
				optional: undefined,
				evidence: "comments",
			},
			{
				id: "sub_issues",
				command: "gh",
				args: ["api", "--paginate", "--slurp", "repos/Acme/widget/issues/7/sub_issues?per_page=100"],
				shape: { kind: "gh-slurp" },
				optional: undefined,
				evidence: undefined,
			},
			{
				id: "parent",
				command: "gh",
				args: ["api", "repos/Acme/widget/issues/7/parent"],
				shape: { kind: "json" },
				optional: true,
				evidence: undefined,
			},
		]);
	});

	it("pins PR object, conversation, files, commits, check-runs, and combined status commands", () => {
		const plan = createGithubAdapter(readExec(), "/repo").readPlan(repository, "pr", 12, [
			"artifact",
			"comments",
			"files",
			"commits",
			"check_runs",
			"status",
		]);
		expect(plan.segments.map((segment) => segment.args)).toEqual([
			["api", "repos/Acme/widget/pulls/12"],
			["api", "--paginate", "--slurp", "repos/Acme/widget/issues/12/comments?per_page=100"],
			["api", "--paginate", "--slurp", "repos/Acme/widget/pulls/12/files?per_page=100"],
			["api", "--paginate", "--slurp", "repos/Acme/widget/pulls/12/commits?per_page=100"],
			["api", "--paginate", "--slurp", "repos/Acme/widget/commits/refs%2Fpull%2F12%2Fhead/check-runs?per_page=100"],
			["api", "--paginate", "--slurp", "repos/Acme/widget/commits/refs%2Fpull%2F12%2Fhead/status?per_page=100"],
		]);
		expect(plan.segments.at(-2)?.shape).toEqual({ kind: "gh-slurp", itemsPath: ["check_runs"] });
		expect(plan.segments.at(-1)?.shape).toEqual({ kind: "gh-slurp", itemsPath: ["statuses"] });
	});

	it("reads only decoded mutable artifact and comment fields with strict parent identity", async () => {
		const exec = readExec();
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.readEditable(repository, "issue", 7, { kind: "artifact" })).resolves.toEqual({
			target: { kind: "artifact" },
			kind: "issue",
			number: 7,
			url: issue.html_url,
			title: issue.title,
			body: issue.body,
		});
		await expect(adapter.readEditable(repository, "pr", 12, { kind: "artifact" })).resolves.toMatchObject({
			kind: "pr",
			draft: false,
			head: "feature/parser",
			base: "main",
		});
		await expect(adapter.readEditable(repository, "issue", 7, { kind: "comment", id: "101" })).resolves.toEqual({
			target: { kind: "comment", id: "101" },
			kind: "issue",
			number: 7,
			url: comment.html_url,
			body: comment.body,
		});
		expect(exec).toHaveBeenCalledTimes(3);
	});

	it("rejects wrong kinds, parents, IDs, providers, and unsupported segment combinations before guessing", async () => {
		const exec = vi.fn<ForgeExec>(async (_command, args) => {
			if (args.at(-1) === "repos/Acme/widget/issues/7") return result({ ...issue, pull_request: {} });
			if (args.at(-1) === "repos/Acme/widget/issues/comments/101")
				return result({ ...comment, issue_url: comment.issue_url.replace("/7", "/8") });
			throw new Error("unexpected");
		});
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.readEditable(repository, "issue", 7, { kind: "artifact" })).rejects.toThrow(/GitHub issue/);
		await expect(adapter.readEditable(repository, "issue", 7, { kind: "comment", id: "101" })).rejects.toThrow(
			/GitHub comment/,
		);
		expect(() => adapter.readPlan(repository, "issue", 7, ["files"])).toThrow(/issue reads/);
		expect(() =>
			adapter.readPlan({ forge: "gitlab", host: "gitlab.com", projectPath: "Acme/widget" }, "issue", 7, [
				"artifact",
			]),
		).toThrow(/GitHub repository/);
	});
});

describe("createGithubAdapter mutations", () => {
	it("keeps one shell-free mutation with exact JSON stdin and validates canonical response identity case-insensitively", async () => {
		const exec = vi.fn<ForgeExec>(async (_command, args, options) => {
			if (args.at(-1) === "repos/acme/widget/issues") {
				expect(options?.stdin).toBe(JSON.stringify({ title: "T", body: "B" }));
				return result({ number: 7, html_url: "https://github.com/Acme/widget/issues/7" });
			}
			if (args.at(-1) === "repos/acme/widget/issues/7/comments") {
				expect(options?.stdin).toBe(JSON.stringify({ body: "C" }));
				return result({ ...comment, html_url: "https://github.com/Acme/widget/issues/7#issuecomment-101" });
			}
			throw new Error("unexpected");
		});
		const lower = { ...repository, projectPath: "acme/widget" };
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.createArtifact(lower, { kind: "issue", title: "T", body: "B" })).resolves.toEqual({
			kind: "issue",
			number: 7,
			url: "https://github.com/Acme/widget/issues/7",
		});
		await expect(adapter.addComment(lower, { kind: "issue", number: 7, body: "C" })).resolves.toMatchObject({
			kind: "comment",
			id: "101",
		});
		expect(exec).toHaveBeenCalledTimes(2);
	});

	it("preserves exact artifact and comment update endpoints, payloads, and response identity", async () => {
		const prComment = {
			id: 101,
			html_url: "https://github.com/Acme/widget/pull/12#issuecomment-101",
			issue_url: "https://api.github.com/repos/Acme/widget/issues/12",
			body: "Updated comment",
		};
		const exec = vi.fn<ForgeExec>(async (_command, args, options) => {
			if (args.at(-1) === "repos/Acme/widget/pulls/12") {
				expect(args).toEqual(["api", "--method", "PATCH", "--input", "-", "repos/Acme/widget/pulls/12"]);
				expect(options?.stdin).toBe(JSON.stringify({ title: "Updated" }));
				return result({ number: 12, html_url: pr.html_url });
			}
			if (args.at(-1) === "repos/Acme/widget/issues/comments/101") {
				expect(args).toEqual(["api", "--method", "PATCH", "--input", "-", "repos/Acme/widget/issues/comments/101"]);
				expect(options?.stdin).toBe(JSON.stringify({ body: "Updated comment" }));
				return result(prComment);
			}
			throw new Error("unexpected");
		});
		const adapter = createGithubAdapter(exec, "/repo");
		await expect(adapter.updateArtifact(repository, { kind: "pr", number: 12, title: "Updated" })).resolves.toEqual({
			kind: "pr",
			number: 12,
			url: pr.html_url,
		});
		await expect(
			adapter.updateComment(repository, { kind: "pr", number: 12, id: "101", body: "Updated comment" }),
		).resolves.toEqual({ kind: "comment", id: "101", url: prComment.html_url });
	});
});
