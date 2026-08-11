import type { ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import type { ForgeExec } from "../src/forge/client.js";
import { createGitlabAdapter } from "../src/forge/gitlab.js";
import type { ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = {
	forge: "gitlab",
	host: "gitlab.com",
	projectPath: "Acme/platform/widget",
};

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
	id: 70,
	iid: 7,
	project_id: 9,
	web_url: "https://gitlab.com/Acme/platform/widget/-/work_items/7",
	title: "Parser failure",
	description: "first\tline",
};
const pr = {
	id: 120,
	iid: 12,
	project_id: 9,
	web_url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12",
	title: "Ship parser",
	description: "Ready",
	draft: false,
	source_branch: "feature/parser",
	target_branch: "main",
};
const note = {
	id: 101,
	project_id: 9,
	noteable_id: 70,
	noteable_iid: 7,
	noteable_type: "Issue",
	system: false,
	position: null,
	type: null,
	body: "Investigating",
};

function readExec(): ForgeExec {
	return vi.fn(async (_command, args) => {
		const endpoint = args.at(-1);
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7") return result(issue);
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12") return result(pr);
		if (endpoint === "projects/Acme%2Fplatform%2Fwidget/issues/7/notes/101") return result(note);
		throw new Error(`Unexpected invocation: ${JSON.stringify(args)}`);
	});
}

describe("createGitlabAdapter native reads", () => {
	it("pins nested-namespace object, NDJSON notes, and native hierarchy commands", () => {
		const plan = createGitlabAdapter(readExec(), "/repo").readPlan(repository, "issue", 7, [
			"artifact",
			"comments",
			"relationships",
		]);
		expect(plan.segments[0]).toMatchObject({
			id: "artifact",
			command: "glab",
			args: ["api", "projects/Acme%2Fplatform%2Fwidget/issues/7"],
			shape: { kind: "json" },
			evidence: "artifact",
		});
		expect(plan.segments[1]).toMatchObject({
			id: "comments",
			args: [
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/issues/7/notes?per_page=100&sort=asc&order_by=created_at",
			],
			shape: { kind: "ndjson" },
			evidence: "comments",
		});
		expect(plan.segments[2].args.slice(0, 5)).toEqual(["api", "graphql", "--paginate", "--output", "ndjson"]);
		expect(plan.segments[2].args).toContain("fullPath=Acme/platform/widget");
		expect(plan.segments[2].args).toContain("iid=7");
		expect(plan.segments[2].shape).toEqual({ kind: "ndjson" });
	});

	it("pins merge-request files, commits, and pipelines without normalization count probes", () => {
		const plan = createGitlabAdapter(readExec(), "/repo").readPlan(repository, "pr", 12, [
			"artifact",
			"comments",
			"files",
			"commits",
			"pipelines",
		]);
		expect(plan.segments.map((segment) => segment.args)).toEqual([
			["api", "projects/Acme%2Fplatform%2Fwidget/merge_requests/12"],
			[
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/notes?per_page=100&sort=asc&order_by=created_at",
			],
			[
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/diffs?per_page=100",
			],
			[
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/commits?per_page=100",
			],
			[
				"api",
				"--paginate",
				"--output",
				"ndjson",
				"projects/Acme%2Fplatform%2Fwidget/merge_requests/12/pipelines?per_page=100",
			],
		]);
		expect(plan.segments.every((segment) => !segment.args.includes("--include"))).toBe(true);
	});

	it("reads only decoded editable fields and preserves the top-level-note edit boundary", async () => {
		const adapter = createGitlabAdapter(readExec(), "/repo");
		await expect(adapter.readEditable(repository, "issue", 7, { kind: "artifact" })).resolves.toEqual({
			target: { kind: "artifact" },
			kind: "issue",
			number: 7,
			url: issue.web_url,
			title: issue.title,
			body: issue.description,
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
			url: "https://gitlab.com/Acme/platform/widget/-/work_items/7#note_101",
			body: note.body,
		});
	});

	it.each([{ system: true }, { position: { line: 1 } }, { type: "DiscussionNote" }, { noteable_iid: 8 }])(
		"rejects non-editable or mismatched notes: $system$type$noteable_iid",
		async (override) => {
			const exec = vi.fn<ForgeExec>(async () => result({ ...note, ...override }));
			await expect(
				createGitlabAdapter(exec, "/repo").readEditable(repository, "issue", 7, { kind: "comment", id: "101" }),
			).rejects.toThrow(/GitLab note/);
		},
	);

	it("rejects provider-inapplicable segments rather than normalizing across forge dialects", () => {
		const adapter = createGitlabAdapter(readExec(), "/repo");
		expect(() => adapter.readPlan(repository, "issue", 7, ["sub_issues"])).toThrow(/does not support/);
		expect(() => adapter.readPlan(repository, "pr", 12, ["check_runs"])).toThrow(/does not support/);
		expect(() =>
			adapter.readPlan({ forge: "github", host: "github.com", projectPath: "Acme/widget" }, "issue", 7, [
				"artifact",
			]),
		).toThrow(/GitLab repository/);
	});
});

describe("createGitlabAdapter mutations", () => {
	it("keeps exact stdin mutation transport and requires caller-approved draft prefixes", async () => {
		const exec = vi.fn<ForgeExec>(async (_command, args, options) => {
			if (args.at(-1) === "projects/Acme%2Fplatform%2Fwidget/merge_requests") {
				expect(options?.stdin).toBe(
					JSON.stringify({
						title: "Draft: Ship",
						description: "Body",
						source_branch: "feature",
						target_branch: "main",
					}),
				);
				return result({ iid: 12, project_id: 9, web_url: pr.web_url });
			}
			throw new Error("unexpected");
		});
		const adapter = createGitlabAdapter(exec, "/repo");
		await expect(
			adapter.createArtifact(repository, {
				kind: "pr",
				title: "Draft: Ship",
				body: "Body",
				head: "feature",
				base: "main",
				draft: true,
			}),
		).resolves.toMatchObject({ kind: "pr", number: 12 });
		await expect(
			adapter.createArtifact(repository, {
				kind: "pr",
				title: "Ship",
				body: "Body",
				head: "feature",
				base: "main",
				draft: true,
			}),
		).rejects.toThrow(/exact approved title/);
	});

	it("preserves exact merge-request and note update endpoints, payloads, and response identity", async () => {
		const exec = vi.fn<ForgeExec>(async (_command, args, options) => {
			if (args.at(-1) === "projects/Acme%2Fplatform%2Fwidget/merge_requests/12") {
				expect(args).toEqual([
					"api",
					"--method",
					"PUT",
					"--input",
					"-",
					"projects/Acme%2Fplatform%2Fwidget/merge_requests/12",
				]);
				expect(options?.stdin).toBe(JSON.stringify({ description: "Updated" }));
				return result({ iid: 12, project_id: 9, web_url: pr.web_url });
			}
			if (args.at(-1) === "projects/Acme%2Fplatform%2Fwidget/issues/7/notes/101") {
				expect(options?.stdin).toBe(JSON.stringify({ body: "Updated note" }));
				return result(note);
			}
			throw new Error("unexpected");
		});
		const adapter = createGitlabAdapter(exec, "/repo");
		await expect(adapter.updateArtifact(repository, { kind: "pr", number: 12, body: "Updated" })).resolves.toEqual({
			kind: "pr",
			number: 12,
			url: pr.web_url,
		});
		await expect(
			adapter.updateComment(repository, { kind: "issue", number: 7, id: "101", body: "Updated note" }),
		).resolves.toEqual({
			kind: "comment",
			id: "101",
			url: "https://gitlab.com/Acme/platform/widget/-/work_items/7#note_101",
		});
	});
});
