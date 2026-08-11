import { readFileSync } from "node:fs";
import type { ExecResult } from "@leanandmean/coding-agent";
import { Value } from "typebox/value";
import { describe, expect, it, vi } from "vitest";
import { filterForgeReply } from "../src/forge/filter.js";
import { registerForgeTools } from "../src/forge/tools.js";
import type {
	ForgeAdapter,
	ForgeEditable,
	ForgeReadDetails,
	ForgeReadPlan,
	ForgeReadSegmentId,
	ForgeRepository,
} from "../src/forge/types.js";
import { recordingPi } from "./helpers.js";

const githubRepository: ForgeRepository = { forge: "github", host: "github.com", projectPath: "Acme/widget" };
const gitlabRepository: ForgeRepository = { forge: "gitlab", host: "gitlab.com", projectPath: "Acme/platform/widget" };

function execResult(stdout = "", overrides: Partial<ExecResult> = {}): ExecResult {
	return { stdout, stderr: "", code: 0, killed: false, ...overrides };
}

function fixtureText(name: string): string {
	return readFileSync(new URL(`fixtures/forge/${name}`, import.meta.url), "utf8");
}

function artifactEditable(overrides: Partial<Extract<ForgeEditable, { target: { kind: "artifact" } }>> = {}) {
	return {
		target: { kind: "artifact" as const },
		kind: "issue" as const,
		number: 7,
		url: "https://github.com/Acme/widget/issues/7",
		title: "Parser failure",
		body: "first\tline",
		...overrides,
	};
}

function planFor(
	repository: ForgeRepository,
	kind: "issue" | "pr",
	number: number,
	include: readonly ForgeReadSegmentId[],
): ForgeReadPlan {
	return {
		repository,
		artifact: { kind, number },
		include: [...include],
		segments: include.map((id) => ({
			id,
			command: repository.forge === "github" ? "gh" : "glab",
			args: ["api", `${kind}/${number}/${id}`],
			shape:
				id === "artifact"
					? { kind: "json" as const }
					: repository.forge === "github"
						? { kind: "gh-slurp" as const }
						: { kind: "ndjson" as const },
			...(id === "artifact" || id === "comments" ? { evidence: id } : {}),
		})),
	};
}

function adapter(overrides: Partial<ForgeAdapter> = {}): ForgeAdapter {
	return {
		readPlan: planFor,
		readEditable: vi.fn(async (_repository, _kind, _number, target) => {
			if (target.kind === "artifact") return artifactEditable();
			return {
				target,
				kind: "issue",
				number: 7,
				url: `https://github.com/Acme/widget/issues/7#issuecomment-${target.id}`,
				body: "comment body",
			};
		}),
		createArtifact: vi.fn(),
		updateArtifact: vi.fn(),
		addComment: vi.fn(),
		updateComment: vi.fn(),
		...overrides,
	};
}

function setup(
	repository = githubRepository,
	overrides: Partial<ForgeAdapter> = {},
	exec?: (command: string, args: string[]) => Promise<ExecResult>,
) {
	const bag = recordingPi();
	bag.pi.exec = vi.fn(
		exec ??
			(async (_command, args) => {
				const id = args.at(-1)?.split("/").at(-1);
				if (id === "artifact") return execResult(JSON.stringify({ number: 7, title: "Parser", body: "Body" }));
				return execResult(repository.forge === "github" ? "[[]]" : "");
			}),
	);
	const forgeAdapter = adapter(overrides);
	registerForgeTools(bag.pi, {
		resolveRepository: async () => repository,
		createAdapter: () => forgeAdapter,
	});
	return {
		bag,
		adapter: forgeAdapter,
		readIssue: bag.tools.find((tool) => tool.name === "read_issue"),
		readPr: bag.tools.find((tool) => tool.name === "read_pr"),
		createIssue: bag.tools.find((tool) => tool.name === "create_issue"),
		addIssueComment: bag.tools.find((tool) => tool.name === "add_issue_comment"),
		editIssue: bag.tools.find((tool) => tool.name === "edit_issue"),
		editPr: bag.tools.find((tool) => tool.name === "edit_pr"),
	};
}

function context(entries: unknown[] = []) {
	return { cwd: "/repo", sessionManager: { getBranch: () => entries } };
}

let entryId = 0;
function entry(message: unknown) {
	return { type: "message", id: `entry-${++entryId}`, parentId: null, timestamp: "2026-01-01", message };
}
function assistantCall(id: string, name: string) {
	return entry({ role: "assistant", content: [{ type: "toolCall", id, name, arguments: {} }] });
}
function toolResult(id: string, name: string, details: unknown, isError = false, content?: string) {
	const readDetails = details as Partial<ForgeReadDetails>;
	const text =
		content ??
		(readDetails.schema === "scramjet:forge-read@2" && Array.isArray(readDetails.segments)
			? "\n".repeat(readDetails.segments.length)
			: "read");
	return entry({
		role: "toolResult",
		toolCallId: id,
		toolName: name,
		content: [{ type: "text", text }],
		details,
		isError,
	});
}

function receipt(
	roles: Array<"artifact" | "comments">,
	repository = githubRepository,
	artifact: { kind: "issue" | "pr"; number: number } = { kind: "issue", number: 7 },
): ForgeReadDetails {
	return {
		schema: "scramjet:forge-read@2",
		repository,
		artifact,
		snapshot: "a".repeat(64),
		include: roles,
		segments: roles.map((role, index) => ({
			id: role,
			status: "ok",
			snapshot: String(index + 1).repeat(64),
			evidence: role,
			coverage: {
				unit: "items",
				offset: 1,
				count: role === "comments" ? 0 : 1,
				totalItems: role === "comments" ? 0 : 1,
			},
			payload: { segment: { start: index, end: index + 1 }, output: { start: index, end: index } },
		})),
	};
}

function evidenceBranch(receipts: ForgeReadDetails[], currentId: string, currentTool: string) {
	const readTool = currentTool.endsWith("_pr") ? "read_pr" : "read_issue";
	return [
		...receipts.flatMap((details, index) => [
			assistantCall(`read-${index}`, readTool),
			toolResult(`read-${index}`, readTool, details),
		]),
		assistantCall(currentId, currentTool),
	];
}

async function expectFailure(result: Promise<any>, failureClass: string, message?: RegExp) {
	const value = await result;
	expect(value).toMatchObject({ isError: true, details: { schema: "scramjet:forge-failure@1", class: failureClass } });
	if (message) expect(value.content[0].text).toMatch(message);
	return value;
}

describe("registerForgeTools native read contracts", () => {
	it("registers eight independent tools with strict generalized segment schemas", () => {
		const { bag, readIssue, readPr, editIssue } = setup();
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
		for (const tool of [readIssue, readPr]) {
			expect(tool.description).toContain("native gh/glab JSON");
			expect(
				Value.Check(tool.parameters, { number: 7, include: ["comments"], offset: 2, snapshot: "a".repeat(64) }),
			).toBe(true);
			expect(Value.Check(tool.parameters, { number: 7, include: ["comments", "comments"] })).toBe(false);
			expect(Value.Check(tool.parameters, { number: 7, include: ["unknown"] })).toBe(false);
		}
		expect(
			Value.Check(readIssue.parameters, {
				number: 7,
				byte_offset: 2,
				snapshot: "a".repeat(64),
				include: ["artifact"],
			}),
		).toBe(true);
		expect(editIssue.description).toContain("JSON-decoded provider field values");
	});

	it.each([
		[githubRepository, "issue", "read_issue", "artifact", fixtureText("github-native-issue.json")],
		[githubRepository, "pr", "read_pr", "artifact", fixtureText("github-native-pr.json")],
		[gitlabRepository, "issue", "read_issue", "artifact", fixtureText("gitlab-issue.json")],
		[gitlabRepository, "pr", "read_pr", "artifact", fixtureText("gitlab-pr.json")],
	] as const)(
		"returns the exact pinned-command echo plus delete-only filtered %s %s captured reply",
		async (repository, _kind, toolName, segment, raw) => {
			const { bag } = setup(repository, {}, async () => execResult(raw));
			const tool = bag.tools.find((candidate) => candidate.name === toolName);
			const result = await tool.execute(
				"read",
				{ number: toolName === "read_pr" ? 12 : 7, include: [segment] },
				undefined,
				undefined,
				context(),
			);
			const command = repository.forge === "github" ? "gh" : "glab";
			const kind = toolName === "read_pr" ? "pr" : "issue";
			expect(result.content[0].text).toBe(
				`$ ${command} api ${kind}/${toolName === "read_pr" ? 12 : 7}/artifact\n${filterForgeReply(raw, repository.forge, "artifact")}\n`,
			);
		},
	);

	it("filters GitLab aggregate comments to ordinary top-level notes", async () => {
		const raw = (JSON.parse(fixtureText("gitlab-issue-notes.json")) as unknown[])
			.map((note) => JSON.stringify(note))
			.join("\n");
		const { readIssue } = setup(gitlabRepository, {}, async () => execResult(raw));
		const result = await readIssue.execute(
			"read",
			{ number: 7, include: ["comments"] },
			undefined,
			undefined,
			context(),
		);
		const output = result.content[0].text;
		expect(output).toContain('"id":101');
		expect(output).toContain('"id":202');
		for (const excluded of [303, 404, 505]) expect(output).not.toContain(`"id":${excluded}`);
	});

	it("fetches once, snapshots once, and serves continuation windows from cache", async () => {
		const comments = [Array.from({ length: 50 }, (_, index) => ({ id: index, body: "x".repeat(1800) }))];
		const { readIssue, bag } = setup(githubRepository, {}, async () => execResult(JSON.stringify(comments)));
		const first = await readIssue.execute(
			"read-1",
			{ number: 7, include: ["comments"] },
			undefined,
			undefined,
			context(),
		);
		expect(first.content[0].text).toContain('continue with include=["comments"]');
		const coverage = first.details.segments[0].coverage;
		if (coverage.unit !== "items") throw new Error("expected items");
		const second = await readIssue.execute(
			"read-2",
			{
				number: 7,
				include: ["comments"],
				offset: coverage.offset + coverage.count,
				snapshot: first.details.snapshot,
			},
			undefined,
			undefined,
			context(),
		);
		expect(second.content[0].text.startsWith("$ gh")).toBe(false);
		expect(bag.pi.exec).toHaveBeenCalledTimes(1);
	});

	it("persists optional-command failures visibly while malformed required shapes retain actionable segment context", async () => {
		const parentPlan: ForgeAdapter["readPlan"] = (repository, kind, number) => ({
			repository,
			artifact: { kind, number },
			include: ["parent"],
			segments: [{ id: "parent", command: "gh", args: ["api", "parent"], shape: { kind: "json" }, optional: true }],
		});
		const unauthorized = setup(githubRepository, { readPlan: parentPlan }, async () =>
			execResult("", { code: 1, stderr: "HTTP 401 Unauthorized" }),
		);
		const optional = await unauthorized.readIssue.execute(
			"read",
			{ number: 7, include: ["parent"] },
			undefined,
			undefined,
			context(),
		);
		expect(optional.isError).toBeUndefined();
		expect(optional.content[0].text).toContain("$ gh api parent\nHTTP 401 Unauthorized");
		expect(optional.details.segments[0].status).toBe("optional_error");

		const malformed = setup(githubRepository, {}, async () => execResult("not json"));
		await expectFailure(
			malformed.readIssue.execute("read", { number: 7, include: ["artifact"] }, undefined, undefined, context()),
			"FORGE_READ_FAILED",
			/Forge segment artifact from gh api issue\/7\/artifact.*json reply contract/,
		);
	});

	it("renders without refetching or mutating persisted evidence and reuses clean component state", async () => {
		const { readIssue, bag } = setup(
			githubRepository,
			{
				readPlan(repository, kind, number) {
					return {
						repository,
						artifact: { kind, number },
						include: ["parent"],
						segments: [
							{ id: "parent", command: "gh", args: ["api", "parent"], shape: { kind: "json" }, optional: true },
						],
					};
				},
			},
			async () =>
				execResult('{"message":"none"}', { code: 1, stderr: "\u202Espoof\ngh: No parent issue found (HTTP 404)" }),
		);
		const result = await readIssue.execute(
			"read",
			{ number: 7, include: ["parent"] },
			undefined,
			undefined,
			context(),
		);
		expect(result.content[0].text).toContain("No parent issue found");
		const beforeContent = structuredClone(result.content);
		const beforeDetails = structuredClone(result.details);
		const renderTheme = {
			fg: (_color: string, value: string) => value,
			bold: (value: string) => value,
		} as never;
		const expanded = readIssue.renderResult(result, { expanded: true, isPartial: false }, renderTheme, {
			args: { number: 7 },
		} as never);
		const expandedText = expanded.render(80).join("\n");
		expect(expandedText).not.toContain("\u202E");
		expect(expandedText).toContain("\\u202E");
		expect(expandedText).toContain("$ gh api parent");
		expect(expandedText).toContain("Provider error");
		const collapsed = readIssue.renderResult(result, { expanded: false, isPartial: false }, renderTheme, {
			args: { number: 7 },
			lastComponent: expanded,
		} as never);
		expect(collapsed).toBe(expanded);
		const collapsedText = collapsed.render(80).join("\n");
		expect(collapsedText).toContain("parent error");
		expect(collapsedText).not.toContain("No parent issue found");
		const partial = readIssue.renderResult(result, { expanded: false, isPartial: true }, renderTheme, {
			args: { number: 7 },
			lastComponent: collapsed,
		} as never);
		expect(partial).toBe(expanded);
		expect(partial.render(80).join("\n").trim()).toBe("Reading forge artifact...");
		const reexpanded = readIssue.renderResult(result, { expanded: true, isPartial: false }, renderTheme, {
			args: { number: 7 },
			lastComponent: partial,
		} as never);
		expect(reexpanded).toBe(expanded);
		const reexpandedText = reexpanded.render(80).join("\n");
		expect(reexpandedText).toContain("Raw transcript");
		expect(reexpandedText).toContain("Provider error");
		expect(reexpandedText).not.toContain("Reading forge artifact...");
		expect(result.content).toEqual(beforeContent);
		expect(result.details).toEqual(beforeDetails);
		expect(bag.pi.exec).toHaveBeenCalledTimes(1);
	});
});

describe("segment-scoped mutation evidence and flows", () => {
	it("requires artifact plus comments evidence for comment creation, then performs one write and one verification reread", async () => {
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async (_repository, _kind, _number, target) =>
			target.kind === "artifact"
				? artifactEditable()
				: {
						target,
						kind: "issue",
						number: 7,
						url: "https://github.com/Acme/widget/issues/7#issuecomment-501",
						body: "New",
					},
		);
		const addComment = vi.fn<ForgeAdapter["addComment"]>(async () => ({
			kind: "comment",
			id: "501",
			url: "https://github.com/Acme/widget/issues/7#issuecomment-501",
		}));
		const { addIssueComment } = setup(githubRepository, { readEditable, addComment });
		await expectFailure(
			addIssueComment.execute(
				"add",
				{ number: 7, body: "New" },
				undefined,
				undefined,
				context(evidenceBranch([receipt(["artifact"])], "add", "add_issue_comment")),
			),
			"FORGE_PREFLIGHT_FAILED",
			/artifact and comments/,
		);
		const result = await addIssueComment.execute(
			"add",
			{ number: 7, body: "New" },
			undefined,
			undefined,
			context(evidenceBranch([receipt(["artifact", "comments"])], "add", "add_issue_comment")),
		);
		expect(result.content[0].text).toContain("issuecomment-501");
		expect(addComment).toHaveBeenCalledTimes(1);
		expect(readEditable).toHaveBeenCalledTimes(2);
		expect(result.details.invalidates).toEqual(["comments"]);
	});

	it("authorizes mutation only from complete real continuation receipts under one segment snapshot", async () => {
		const comments = [Array.from({ length: 50 }, (_, index) => ({ id: index + 1, body: "x".repeat(1800) }))];
		let body = "comment body";
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async (_repository, _kind, _number, target) => ({
			target: target.kind === "comment" ? target : { kind: "comment", id: "1" },
			kind: "issue",
			number: 7,
			url: "https://github.com/Acme/widget/issues/7#issuecomment-1",
			body,
		}));
		const updateComment = vi.fn<ForgeAdapter["updateComment"]>(async (_repository, input) => {
			body = input.body;
			return { kind: "comment", id: input.id, url: "https://github.com/Acme/widget/issues/7#issuecomment-1" };
		});
		const { readIssue, editIssue } = setup(githubRepository, { readEditable, updateComment }, async () =>
			execResult(JSON.stringify(comments)),
		);
		const first = await readIssue.execute(
			"read-1",
			{ number: 7, include: ["comments"] },
			undefined,
			undefined,
			context(),
		);
		const firstCoverage = first.details.segments[0].coverage;
		if (firstCoverage.unit !== "items") throw new Error("expected item coverage");
		const second = await readIssue.execute(
			"read-2",
			{
				number: 7,
				include: ["comments"],
				offset: firstCoverage.offset + firstCoverage.count,
				snapshot: first.details.snapshot,
			},
			undefined,
			undefined,
			context(),
		);
		const editArgs = {
			number: 7,
			target: { kind: "comment" as const, id: "1" },
			edits: [{ field: "body" as const, oldText: "comment", newText: "review" }],
		};
		const branch = (secondDetails: ForgeReadDetails, currentId: string) => [
			assistantCall("read-1", "read_issue"),
			toolResult("read-1", "read_issue", first.details, false, first.content[0].text),
			...(secondDetails === first.details
				? []
				: [
						assistantCall("read-2", "read_issue"),
						toolResult("read-2", "read_issue", secondDetails, false, second.content[0].text),
					]),
			assistantCall(currentId, "edit_issue"),
		];
		await expectFailure(
			editIssue.execute("partial", editArgs, undefined, undefined, context(branch(first.details, "partial"))),
			"FORGE_PREFLIGHT_FAILED",
		);
		const mismatched = structuredClone(second.details) as ForgeReadDetails;
		mismatched.segments[0].snapshot = "f".repeat(64);
		await expectFailure(
			editIssue.execute("mismatch", editArgs, undefined, undefined, context(branch(mismatched, "mismatch"))),
			"FORGE_PREFLIGHT_FAILED",
		);
		await expect(
			editIssue.execute("complete", editArgs, undefined, undefined, context(branch(second.details, "complete"))),
		).resolves.toMatchObject({ details: { verified: true } });
		expect(body).toBe("review body");
	});

	it("edits JSON-decoded artifact text with one write and unchanged ambiguity taxonomy", async () => {
		let current = artifactEditable();
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async () => current);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
			current = artifactEditable({ title: input.title ?? current.title, body: input.body ?? current.body });
			return { kind: "issue", number: 7, url: current.url };
		});
		const { editIssue } = setup(githubRepository, { readEditable, updateArtifact });
		const branch = evidenceBranch([receipt(["artifact"])], "edit", "edit_issue");
		const result = await editIssue.execute(
			"edit",
			{ number: 7, target: { kind: "artifact" }, edits: [{ field: "body", oldText: "\t", newText: " " }] },
			undefined,
			undefined,
			context(branch),
		);
		expect(updateArtifact).toHaveBeenCalledTimes(1);
		expect(readEditable).toHaveBeenCalledTimes(2);
		expect(current.body).toBe("first line");
		expect(result.details.invalidates).toEqual(["artifact"]);

		updateArtifact.mockRejectedValueOnce(new Error("response lost"));
		const ambiguous = await expectFailure(
			editIssue.execute(
				"edit-2",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "Parser", newText: "Reader" }],
				},
				undefined,
				undefined,
				context(evidenceBranch([receipt(["artifact"])], "edit-2", "edit_issue")),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/DO NOT RETRY/,
		);
		expect(ambiguous.details).toMatchObject({ artifact: { kind: "issue", number: 7 }, invalidates: ["artifact"] });
	});

	it("classifies post-write verification mismatch as ambiguous without a second write", async () => {
		const original = artifactEditable();
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async () => original);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async () => ({
			kind: "issue",
			number: 7,
			url: original.url,
		}));
		const { editIssue } = setup(githubRepository, { readEditable, updateArtifact });
		const result = await expectFailure(
			editIssue.execute(
				"verify-mismatch",
				{
					number: 7,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "Parser", newText: "Reader" }],
				},
				undefined,
				undefined,
				context(evidenceBranch([receipt(["artifact"])], "verify-mismatch", "edit_issue")),
			),
			"FORGE_WRITE_AMBIGUOUS",
			/may have succeeded/i,
		);
		expect(result.details.phase).toBe("verify");
		expect(updateArtifact).toHaveBeenCalledTimes(1);
		expect(readEditable).toHaveBeenCalledTimes(2);
	});

	it("edits and verifies one comment body without touching artifact content", async () => {
		let body = "comment body";
		const url = "https://github.com/Acme/widget/issues/7#issuecomment-101";
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async (_repository, _kind, _number, target) => ({
			target: target.kind === "comment" ? target : { kind: "comment", id: "101" },
			kind: "issue",
			number: 7,
			url,
			body,
		}));
		const updateComment = vi.fn<ForgeAdapter["updateComment"]>(async (_repository, input) => {
			body = input.body;
			return { kind: "comment", id: input.id, url };
		});
		const { editIssue } = setup(githubRepository, { readEditable, updateComment });
		const result = await editIssue.execute(
			"comment-edit",
			{
				number: 7,
				target: { kind: "comment", id: "101" },
				edits: [{ field: "body", oldText: "comment", newText: "review" }],
			},
			undefined,
			undefined,
			context(evidenceBranch([receipt(["comments"])], "comment-edit", "edit_issue")),
		);
		expect(body).toBe("review body");
		expect(updateComment).toHaveBeenCalledTimes(1);
		expect(readEditable).toHaveBeenCalledTimes(2);
		expect(result.details.invalidates).toEqual(["comments"]);
	});

	it("serializes same-comment refetch, write, and verification as one queue transaction", async () => {
		let body = "one";
		let releaseFirst!: () => void;
		const firstMayFinish = new Promise<void>((resolve) => {
			releaseFirst = resolve;
		});
		const url = "https://github.com/Acme/widget/issues/7#issuecomment-101";
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async (_repository, _kind, _number, target) => ({
			target: target.kind === "comment" ? target : { kind: "comment", id: "101" },
			kind: "issue",
			number: 7,
			url,
			body,
		}));
		let writes = 0;
		const updateComment = vi.fn<ForgeAdapter["updateComment"]>(async (_repository, input) => {
			writes++;
			if (writes === 1) await firstMayFinish;
			body = input.body;
			return { kind: "comment", id: input.id, url };
		});
		const { editIssue } = setup(githubRepository, { readEditable, updateComment });
		const first = editIssue.execute(
			"queue-1",
			{
				number: 7,
				target: { kind: "comment", id: "101" },
				edits: [{ field: "body", oldText: "one", newText: "first" }],
			},
			undefined,
			undefined,
			context(evidenceBranch([receipt(["comments"])], "queue-1", "edit_issue")),
		);
		const second = editIssue.execute(
			"queue-2",
			{
				number: 7,
				target: { kind: "comment", id: "101" },
				edits: [{ field: "body", oldText: "first", newText: "second" }],
			},
			undefined,
			undefined,
			context(evidenceBranch([receipt(["comments"])], "queue-2", "edit_issue")),
		);
		await new Promise((resolve) => setTimeout(resolve, 0));
		expect(readEditable).toHaveBeenCalledTimes(1);
		releaseFirst();
		await Promise.all([first, second]);
		expect(body).toBe("second");
		expect(updateComment).toHaveBeenCalledTimes(2);
		expect(readEditable).toHaveBeenCalledTimes(4);
	});

	it("rejects and verifies GitLab draft-state changes outside the edit surface", async () => {
		const original: ForgeEditable = {
			target: { kind: "artifact" },
			kind: "pr",
			number: 12,
			url: "https://gitlab.com/Acme/platform/widget/-/merge_requests/12",
			title: "Ship",
			body: "Body",
			draft: false,
			head: "feature",
			base: "main",
		};
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async () => original);
		const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>();
		const { editPr } = setup(gitlabRepository, { readEditable, updateArtifact });
		await expectFailure(
			editPr.execute(
				"draft-edit",
				{
					number: 12,
					target: { kind: "artifact" },
					edits: [{ field: "title", oldText: "Ship", newText: "Draft: Ship" }],
				},
				undefined,
				undefined,
				context(
					evidenceBranch(
						[receipt(["artifact"], gitlabRepository, { kind: "pr", number: 12 })],
						"draft-edit",
						"edit_pr",
					),
				),
			),
			"FORGE_PREFLIGHT_FAILED",
			/preserve the existing draft state/,
		);
		expect(updateArtifact).not.toHaveBeenCalled();
	});

	it("invalidates only the mutated role in chronological persisted evidence", async () => {
		const mutation = {
			schema: "scramjet:forge-mutation@1",
			repository: githubRepository,
			artifact: { kind: "issue", number: 7 },
			invalidates: ["artifact"],
			verified: true,
		};
		const branch = [
			assistantCall("read", "read_issue"),
			toolResult("read", "read_issue", receipt(["artifact", "comments"])),
			assistantCall("old-edit", "edit_issue"),
			toolResult("old-edit", "edit_issue", mutation),
			assistantCall("edit", "edit_issue"),
		];
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>();
		const { editIssue } = setup(githubRepository, { readEditable });
		await expectFailure(
			editIssue.execute(
				"edit",
				{ number: 7, target: { kind: "artifact" }, edits: [{ field: "title", oldText: "old", newText: "new" }] },
				undefined,
				undefined,
				context(branch),
			),
			"FORGE_PREFLIGHT_FAILED",
			/artifact segment evidence/,
		);
		expect(readEditable).not.toHaveBeenCalled();
	});

	it("ignores malformed historical invalidation identities while retaining valid read evidence", async () => {
		const malformed = [
			{
				schema: "scramjet:forge-mutation@1",
				repository: null,
				artifact: { kind: "issue", number: 7 },
				invalidates: ["artifact"],
			},
			{
				schema: "scramjet:forge-failure@1",
				repository: githubRepository,
				artifact: null,
				invalidates: ["artifact"],
			},
		];
		for (const [index, details] of malformed.entries()) {
			let current = artifactEditable();
			const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async () => current);
			const updateArtifact = vi.fn<ForgeAdapter["updateArtifact"]>(async (_repository, input) => {
				current = artifactEditable({ title: input.title ?? current.title });
				return { kind: "issue", number: 7, url: current.url };
			});
			const branch = [
				assistantCall(`read-${index}`, "read_issue"),
				toolResult(`read-${index}`, "read_issue", receipt(["artifact"])),
				assistantCall(`history-${index}`, "edit_issue"),
				toolResult(`history-${index}`, "edit_issue", details),
				assistantCall(`current-${index}`, "edit_issue"),
			];
			const { editIssue } = setup(githubRepository, { readEditable, updateArtifact });
			await expect(
				editIssue.execute(
					`current-${index}`,
					{
						number: 7,
						target: { kind: "artifact" },
						edits: [{ field: "title", oldText: "Parser", newText: `Reader ${index}` }],
					},
					undefined,
					undefined,
					context(branch),
				),
			).resolves.toMatchObject({ details: { verified: true } });
		}
	});

	it("creates and verifies through slim editable rereads without returning submitted content", async () => {
		const createArtifact = vi.fn<ForgeAdapter["createArtifact"]>(async () => ({
			kind: "issue",
			number: 7,
			url: artifactEditable().url,
		}));
		const readEditable = vi.fn<ForgeAdapter["readEditable"]>(async () =>
			artifactEditable({ title: "Approved", body: "Secret body" }),
		);
		const { createIssue } = setup(githubRepository, { createArtifact, readEditable });
		const result = await createIssue.execute(
			"create",
			{ title: "Approved", body: "Secret body" },
			undefined,
			undefined,
			context(),
		);
		expect(result.content[0].text).toBe(artifactEditable().url);
		expect(JSON.stringify(result.details)).not.toContain("Secret body");
		expect(createArtifact).toHaveBeenCalledTimes(1);
		expect(readEditable).toHaveBeenCalledTimes(1);
	});
});
