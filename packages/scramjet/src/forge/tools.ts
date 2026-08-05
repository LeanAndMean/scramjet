import { StringEnum } from "@leanandmean/ai";
import type {
	AgentToolResult,
	ExecResult,
	ExtensionAPI,
	SessionEntry,
	Theme,
	ToolRenderResultOptions,
} from "@leanandmean/coding-agent";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES } from "@leanandmean/coding-agent";
import { Text } from "@leanandmean/tui";
import { type Static, Type } from "typebox";
import {
	FORGE_EXEC_TIMEOUT_MS,
	ForgeCommandError,
	type ForgeExec,
	resolveCurrentRepository,
	withForgeMutationQueue,
} from "./client.js";
import { type ForgeRangeRequest, isForgeReadDetails, renderForgeDocument, sliceForgeDocument } from "./document.js";
import { createGithubAdapter } from "./github.js";
import { createGitlabAdapter } from "./gitlab.js";
import type {
	ForgeAdapter,
	ForgeArtifact,
	ForgeArtifactKind,
	ForgeCreateInput,
	ForgeIdentity,
	ForgePrSection,
	ForgeReadDetails,
	ForgeRepository,
} from "./types.js";

const ARTIFACT_NUMBER = Type.Integer({ minimum: 1, description: "Issue or pull request number" });
const OFFSET = Type.Optional(Type.Integer({ minimum: 1, description: "1-indexed XML line to start reading from" }));
const LIMIT = Type.Optional(Type.Integer({ minimum: 1, description: "Maximum XML lines to return" }));
const SNAPSHOT = Type.Optional(
	Type.String({
		pattern: "^[a-f0-9]{64}$",
		description: "Snapshot from the preceding range; fails if the remote artifact changed",
	}),
);

export const READ_ISSUE_SCHEMA = Type.Object(
	{
		number: ARTIFACT_NUMBER,
		offset: OFFSET,
		limit: LIMIT,
		snapshot: SNAPSHOT,
	},
	{ additionalProperties: false },
);

export const READ_PR_SCHEMA = Type.Object(
	{
		number: ARTIFACT_NUMBER,
		include: Type.Optional(
			Type.Array(StringEnum(["files", "commits", "checks"] as const), {
				uniqueItems: true,
				maxItems: 3,
				description: "Optional bulky sections; readiness and conversation comments are always included",
			}),
		),
		offset: OFFSET,
		limit: LIMIT,
		snapshot: SNAPSHOT,
	},
	{ additionalProperties: false },
);

const TITLE = Type.String({ minLength: 1, description: "Artifact title" });
const BODY = Type.String({ description: "Exact Markdown body" });
const COMMENT_BODY = Type.String({ minLength: 1, description: "Exact Markdown comment body" });
const BRANCH = Type.String({ minLength: 1, description: "Explicit branch name" });

export const CREATE_ISSUE_SCHEMA = Type.Object({ title: TITLE, body: BODY }, { additionalProperties: false });
export const CREATE_PR_SCHEMA = Type.Object(
	{
		title: TITLE,
		body: BODY,
		head: BRANCH,
		base: BRANCH,
		draft: Type.Optional(Type.Boolean({ description: "Create as a draft pull request" })),
	},
	{ additionalProperties: false },
);
export const ADD_ISSUE_COMMENT_SCHEMA = Type.Object(
	{ number: ARTIFACT_NUMBER, body: COMMENT_BODY },
	{ additionalProperties: false },
);
export const ADD_PR_COMMENT_SCHEMA = Type.Object(
	{ number: ARTIFACT_NUMBER, body: COMMENT_BODY },
	{ additionalProperties: false },
);

type ReadIssueParams = Static<typeof READ_ISSUE_SCHEMA>;
type ReadPrParams = Static<typeof READ_PR_SCHEMA>;
type CreateIssueParams = Static<typeof CREATE_ISSUE_SCHEMA>;
type CreatePrParams = Static<typeof CREATE_PR_SCHEMA>;
type AddCommentParams = Static<typeof ADD_ISSUE_COMMENT_SCHEMA>;

type ForgeAdapterFactory = (repository: ForgeRepository, exec: ForgeExec, cwd: string) => ForgeAdapter;

export interface ForgeToolDependencies {
	resolveRepository?: typeof resolveCurrentRepository;
	createAdapter?: ForgeAdapterFactory;
}

function defaultAdapter(repository: ForgeRepository, exec: ForgeExec, cwd: string): ForgeAdapter {
	return repository.forge === "github" ? createGithubAdapter(exec, cwd) : createGitlabAdapter(exec, cwd);
}

function forgeExec(pi: ExtensionAPI): ForgeExec {
	return (command, args, options) => pi.exec(command, args, options);
}

function readDescription(kind: ForgeArtifactKind): string {
	const label = kind === "issue" ? "issue" : "pull request";
	return `Read a ${label} from the current repository as deterministic XML, including its complete top-level conversation. Output is bounded to ${DEFAULT_MAX_LINES} lines or ${DEFAULT_MAX_BYTES / 1024}KB; use offset, limit, and the returned snapshot for lossless continuation.`;
}

async function readArtifact(
	kind: ForgeArtifactKind,
	params: ReadIssueParams | ReadPrParams,
	signal: AbortSignal | undefined,
	cwd: string,
	exec: ForgeExec,
	dependencies: Required<ForgeToolDependencies>,
) {
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const include = kind === "pr" ? ([...((params as ReadPrParams).include ?? [])] as ForgePrSection[]) : [];
	const artifact = await adapter.readArtifact(repository, kind, params.number, include, signal);
	if (artifact.kind !== kind || artifact.number !== params.number) {
		throw new Error(`Forge adapter returned the wrong artifact for ${kind} #${params.number}`);
	}
	const request: ForgeRangeRequest = {
		...(params.offset === undefined ? {} : { offset: params.offset }),
		...(params.limit === undefined ? {} : { limit: params.limit }),
		...(params.snapshot === undefined ? {} : { snapshot: params.snapshot }),
	};
	const slice = sliceForgeDocument(renderForgeDocument(repository, artifact), request);
	return {
		content: [{ type: "text" as const, text: slice.content }],
		details: slice.details,
	};
}

function repositoriesEqual(left: ForgeRepository, right: ForgeRepository): boolean {
	return left.forge === right.forge && left.host === right.host && left.projectPath === right.projectPath;
}

function assistantHasToolCall(entry: SessionEntry, toolCallId: string): boolean {
	return (
		entry.type === "message" &&
		entry.message.role === "assistant" &&
		entry.message.content.some((block) => block.type === "toolCall" && block.id === toolCallId)
	);
}

function completeCoverage(total: number, ranges: ForgeReadDetails["core"]["ranges"]): boolean {
	if (total <= 0) return false;
	let covered = 0;
	for (const range of [...ranges].sort((left, right) => left.start - right.start)) {
		if (range.start > covered) return false;
		covered = Math.max(covered, range.end);
		if (covered >= total) return true;
	}
	return false;
}

function hasCompleteParentEvidence(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): boolean {
	let compactionIndex = -1;
	for (let index = 0; index < entries.length; index++) {
		if (entries[index].type === "compaction") compactionIndex = index;
	}
	const currentIndexes = entries
		.map((entry, index) => (assistantHasToolCall(entry, toolCallId) ? index : -1))
		.filter((index) => index > compactionIndex);
	if (currentIndexes.length !== 1) return false;
	const currentIndex = currentIndexes[0];
	const readTool = kind === "issue" ? "read_issue" : "read_pr";
	const readCallIds = new Set<string>();
	const groups = new Map<string, { total: number; ranges: ForgeReadDetails["core"]["ranges"]; valid: boolean }>();

	for (let index = compactionIndex + 1; index < currentIndex; index++) {
		const entry = entries[index];
		if (entry.type !== "message") continue;
		const message = entry.message;
		if (message.role === "assistant") {
			for (const block of message.content) {
				if (block.type === "toolCall" && block.name === readTool) readCallIds.add(block.id);
			}
			continue;
		}
		if (
			message.role !== "toolResult" ||
			message.isError ||
			message.toolName !== readTool ||
			!readCallIds.has(message.toolCallId) ||
			!isForgeReadDetails(message.details)
		) {
			continue;
		}
		const details = message.details;
		if (
			!repositoriesEqual(details.repository, repository) ||
			details.artifact.kind !== kind ||
			details.artifact.number !== number
		) {
			continue;
		}
		const group = groups.get(details.snapshot) ?? { total: details.core.totalLines, ranges: [], valid: true };
		if (group.total !== details.core.totalLines) group.valid = false;
		group.ranges.push(...details.core.ranges);
		groups.set(details.snapshot, group);
	}

	return [...groups.values()].some((group) => group.valid && completeCoverage(group.total, group.ranges));
}

function assertArtifactIdentity(artifact: ForgeArtifact, kind: ForgeArtifactKind, number: number, url?: string): void {
	if (artifact.kind !== kind || artifact.number !== number || (url !== undefined && artifact.url !== url)) {
		throw new Error(`Forge adapter returned the wrong artifact for ${kind} #${number}`);
	}
}

const GITLAB_DRAFT_PREFIX = /^(?:Draft:|\[Draft\]|\(Draft\))\s*/i;

function validateCreateInput(repository: ForgeRepository, input: ForgeCreateInput): void {
	if (repository.forge === "gitlab" && input.kind === "pr" && !input.draft && GITLAB_DRAFT_PREFIX.test(input.title)) {
		throw new Error("GitLab cannot create a non-draft merge request with a draft-prefixed title");
	}
}

function expectedCreatedTitle(repository: ForgeRepository, input: ForgeCreateInput): string {
	if (repository.forge === "gitlab" && input.kind === "pr" && input.draft && !GITLAB_DRAFT_PREFIX.test(input.title)) {
		return `Draft: ${input.title}`;
	}
	return input.title;
}

function verifyCreatedArtifact(repository: ForgeRepository, input: ForgeCreateInput, artifact: ForgeArtifact): void {
	if (artifact.title !== expectedCreatedTitle(repository, input) || artifact.body !== input.body) {
		throw new Error("Created artifact content did not match the requested title and body");
	}
	if (
		input.kind === "pr" &&
		(artifact.kind !== "pr" ||
			artifact.readiness.head !== input.head ||
			artifact.readiness.base !== input.base ||
			artifact.readiness.draft !== input.draft)
	) {
		throw new Error("Created pull request did not match the requested branches and draft state");
	}
}

function mutationResult(
	operation: "create_issue" | "create_pr" | "add_issue_comment" | "add_pr_comment",
	repository: ForgeRepository,
	identity: ForgeIdentity,
	artifact: ForgeArtifact,
	createdCommentBody?: string,
) {
	const slice = sliceForgeDocument(renderForgeDocument(repository, artifact), {});
	const verified =
		identity.kind === "comment"
			? JSON.stringify({ id: identity.id, url: identity.url, body: createdCommentBody })
			: JSON.stringify(identity);
	return {
		content: [{ type: "text" as const, text: `Created and verified ${operation}: ${verified}\n\n${slice.content}` }],
		details: {
			schema: "scramjet:forge-mutation@1" as const,
			operation,
			repository,
			identity,
			verified: true as const,
		},
	};
}

function ambiguousMutation(operation: string, error: unknown): Error {
	const cause = error instanceof Error ? error : new Error(String(error));
	return new Error(
		`${operation} may have succeeded, but its identity or exact content could not be verified. Reread the forge artifact before retrying to avoid a duplicate. ${cause.message}`,
		{ cause },
	);
}

function mutationAttemptFailure(operation: string, error: unknown): Error {
	if (error instanceof ForgeCommandError && (error.kind === "missing-executable" || error.kind === "failed")) {
		return error;
	}
	return ambiguousMutation(operation, error);
}

async function createArtifact(
	kind: ForgeArtifactKind,
	params: CreateIssueParams | CreatePrParams,
	signal: AbortSignal | undefined,
	cwd: string,
	exec: ForgeExec,
	dependencies: Required<ForgeToolDependencies>,
) {
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const input: ForgeCreateInput =
		kind === "issue"
			? { kind, title: params.title, body: params.body }
			: {
					kind,
					title: params.title,
					body: params.body,
					head: (params as CreatePrParams).head,
					base: (params as CreatePrParams).base,
					draft: (params as CreatePrParams).draft ?? false,
				};
	validateCreateInput(repository, input);
	const operation = kind === "issue" ? "Issue creation" : "Pull request creation";
	let identity: Awaited<ReturnType<ForgeAdapter["createArtifact"]>>;
	try {
		identity = await adapter.createArtifact(repository, input, signal);
		if (identity.kind !== kind) throw new Error("Forge adapter returned the wrong created artifact kind");
	} catch (error) {
		throw mutationAttemptFailure(operation, error);
	}
	try {
		const artifact = await adapter.readArtifact(repository, kind, identity.number, [], signal);
		assertArtifactIdentity(artifact, kind, identity.number, identity.url);
		verifyCreatedArtifact(repository, input, artifact);
		return mutationResult(kind === "issue" ? "create_issue" : "create_pr", repository, identity, artifact);
	} catch (error) {
		throw ambiguousMutation(operation, error);
	}
}

async function addComment(
	kind: ForgeArtifactKind,
	params: AddCommentParams,
	toolCallId: string,
	signal: AbortSignal | undefined,
	cwd: string,
	entries: readonly SessionEntry[],
	exec: ForgeExec,
	dependencies: Required<ForgeToolDependencies>,
) {
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	if (!hasCompleteParentEvidence(entries, toolCallId, repository, kind, params.number)) {
		const readTool = kind === "issue" ? "read_issue" : "read_pr";
		throw new Error(
			`${kind === "issue" ? "add_issue_comment" : "add_pr_comment"} requires a complete prior ${readTool} of ${kind} #${params.number} on the active branch after the latest compaction, using all ranges from one unchanged snapshot`,
		);
	}
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const key = `${repository.forge}:${repository.host}:${repository.projectPath}:${kind}:${params.number}`;
	return withForgeMutationQueue(key, async () => {
		const parent = await adapter.readArtifact(repository, kind, params.number, [], signal);
		assertArtifactIdentity(parent, kind, params.number);
		const operation = kind === "issue" ? "Issue comment creation" : "PR comment creation";
		let identity: Awaited<ReturnType<ForgeAdapter["addComment"]>>;
		try {
			identity = await adapter.addComment(repository, { kind, number: params.number, body: params.body }, signal);
		} catch (error) {
			throw mutationAttemptFailure(operation, error);
		}
		try {
			const updated = await adapter.readArtifact(repository, kind, params.number, [], signal);
			assertArtifactIdentity(updated, kind, params.number);
			const comment = updated.comments.find((candidate) => candidate.id === identity.id);
			if (comment === undefined || comment.url !== identity.url || comment.body !== params.body) {
				throw new Error("Created comment did not match its returned identity and requested body");
			}
			return mutationResult(
				kind === "issue" ? "add_issue_comment" : "add_pr_comment",
				repository,
				identity,
				updated,
				params.body,
			);
		} catch (error) {
			throw ambiguousMutation(operation, error);
		}
	});
}

interface ForgeRenderContext {
	lastComponent?: unknown;
}

function renderCall(
	name: "read_issue" | "read_pr",
	args: ReadIssueParams | ReadPrParams,
	theme: Theme,
	context: ForgeRenderContext,
) {
	const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
	let content = theme.fg("toolTitle", theme.bold(`${name} `)) + theme.fg("accent", `#${args.number}`);
	const options: string[] = [];
	if ("include" in args && args.include?.length) options.push(`include=${args.include.join(",")}`);
	if (args.offset !== undefined) options.push(`offset=${args.offset}`);
	if (args.limit !== undefined) options.push(`limit=${args.limit}`);
	if (options.length > 0) content += theme.fg("dim", ` (${options.join(" ")})`);
	text.setText(content);
	return text;
}

function resultText(result: AgentToolResult<unknown>): string {
	const content = result.content.find((item) => item.type === "text");
	return content?.type === "text" ? content.text : "";
}

function renderResult(
	result: AgentToolResult<unknown>,
	options: ToolRenderResultOptions,
	theme: Theme,
	context: ForgeRenderContext,
) {
	const text = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
	if (options.isPartial) {
		text.setText(theme.fg("warning", "Reading forge artifact..."));
		return text;
	}
	if (!isForgeReadDetails(result.details)) {
		text.setText(resultText(result));
		return text;
	}
	const { offset, lines, totalLines } = result.details.range;
	const end = offset + lines - 1;
	let content = theme.fg("success", `lines ${offset}-${end} of ${totalLines}`);
	if (options.expanded) {
		const persisted = resultText(result);
		if (persisted !== "") content += `\n${theme.fg("toolOutput", persisted)}`;
	}
	text.setText(content);
	return text;
}

const AUTH_FAILURE = [
	/\bnot logged (?:in|into)\b/i,
	/\bno hosts? (?:are )?configured\b/i,
	/\bno (?:authentication |auth )?token (?:is )?found\b/i,
	/\btoken\b[^\n]*(?:invalid|expired|revoked)\b/i,
	/(?:invalid|expired|revoked)[^\n]*\btoken\b/i,
	/\bauthentication (?:failed|required)\b/i,
	/\bunauthorized\b/i,
	/\bHTTP(?:\/\S+)?\s+401\b/i,
];

function prerequisiteWarning(repository: ForgeRepository, result: ExecResult): string | null {
	const cli = repository.forge === "github" ? "gh" : "glab";
	const name = repository.forge === "github" ? "GitHub CLI" : "GitLab CLI";
	if (result.spawnError?.code === "ENOENT") {
		return `Scramjet forge tools require ${name} (${cli}), but it is not installed.`;
	}
	if (result.code === 0 || result.killed || result.spawnError || result.stdinError) return null;
	const diagnostic = `${result.stdout}\n${result.stderr}`;
	if (!AUTH_FAILURE.some((pattern) => pattern.test(diagnostic))) return null;
	return `Scramjet forge tools cannot authenticate to ${repository.host}. Run \`${cli} auth login --hostname ${repository.host}\`, then restart Scramjet.`;
}

async function probePrerequisite(
	exec: ForgeExec,
	cwd: string,
	resolveRepository: typeof resolveCurrentRepository,
): Promise<string | null> {
	const repository = await resolveRepository(exec, cwd);
	const cli = repository.forge === "github" ? "gh" : "glab";
	const args = ["auth", "status", "--hostname", repository.host];
	if (repository.forge === "github") args.push("--active");
	const result = await exec(cli, args, {
		cwd,
		timeout: FORGE_EXEC_TIMEOUT_MS,
	});
	return prerequisiteWarning(repository, result);
}

export function registerForgeTools(pi: ExtensionAPI, overrides: ForgeToolDependencies = {}): void {
	const dependencies: Required<ForgeToolDependencies> = {
		resolveRepository: overrides.resolveRepository ?? resolveCurrentRepository,
		createAdapter: overrides.createAdapter ?? defaultAdapter,
	};
	const exec = forgeExec(pi);

	pi.registerTool({
		name: "read_issue",
		label: "read issue",
		description: readDescription("issue"),
		promptSnippet: "Read an issue and all top-level comments from the current repository",
		promptGuidelines: [
			"Use read_issue instead of bash or forge CLI commands when reading a current-repository issue.",
			"When read_issue output is truncated, continue with offset and the unchanged snapshot until complete.",
		],
		parameters: READ_ISSUE_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return readArtifact("issue", params, signal, ctx.cwd, exec, dependencies);
		},
		renderCall(args, theme, context) {
			return renderCall("read_issue", args, theme, context);
		},
		renderResult,
	});

	pi.registerTool({
		name: "read_pr",
		label: "read pull request",
		description: readDescription("pr"),
		promptSnippet: "Read a pull request and all top-level comments from the current repository",
		promptGuidelines: [
			"Use read_pr instead of bash or forge CLI commands when reading a current-repository pull request.",
			"When read_pr output is truncated, continue with offset and the unchanged snapshot until complete.",
		],
		parameters: READ_PR_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return readArtifact("pr", params, signal, ctx.cwd, exec, dependencies);
		},
		renderCall(args, theme, context) {
			return renderCall("read_pr", args, theme, context);
		},
		renderResult,
	});

	pi.registerTool({
		name: "create_issue",
		label: "create issue",
		description: "Create and verify an issue in the current repository with an exact title and Markdown body.",
		promptSnippet: "Create an issue in the current repository",
		promptGuidelines: ["Use create_issue only after the user has approved the exact issue title and body."],
		parameters: CREATE_ISSUE_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return createArtifact("issue", params, signal, ctx.cwd, exec, dependencies);
		},
	});

	pi.registerTool({
		name: "create_pr",
		label: "create pull request",
		description:
			"Create and verify a pull request in the current repository with exact content and explicit head/base branches. This never creates, checks out, or pushes branches.",
		promptSnippet: "Create a pull request from explicit existing branches in the current repository",
		promptGuidelines: [
			"Use create_pr only after the user has approved the exact pull request title and body.",
			"Pass explicit existing head and base branches; create_pr never mutates Git state.",
		],
		parameters: CREATE_PR_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return createArtifact("pr", params, signal, ctx.cwd, exec, dependencies);
		},
	});

	pi.registerTool({
		name: "add_issue_comment",
		label: "add issue comment",
		description:
			"Add and verify one top-level issue comment after a complete prior read_issue of the parent conversation.",
		promptSnippet: "Add a top-level comment to a previously read issue in the current repository",
		promptGuidelines: [
			"Before add_issue_comment, completely read the same issue and all top-level comments with read_issue.",
		],
		parameters: ADD_ISSUE_COMMENT_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return addComment(
				"issue",
				params,
				toolCallId,
				signal,
				ctx.cwd,
				ctx.sessionManager.getBranch(),
				exec,
				dependencies,
			);
		},
	});

	pi.registerTool({
		name: "add_pr_comment",
		label: "add pull request comment",
		description:
			"Add and verify one top-level pull request comment after a complete prior read_pr of the parent conversation.",
		promptSnippet: "Add a top-level comment to a previously read pull request in the current repository",
		promptGuidelines: [
			"Before add_pr_comment, completely read the same pull request and all top-level comments with read_pr.",
		],
		parameters: ADD_PR_COMMENT_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return addComment(
				"pr",
				params,
				toolCallId,
				signal,
				ctx.cwd,
				ctx.sessionManager.getBranch(),
				exec,
				dependencies,
			);
		},
	});

	pi.on("session_start", (_event, ctx) => {
		if (!ctx.hasUI) return;
		void probePrerequisite(exec, ctx.cwd, dependencies.resolveRepository)
			.then((warning) => {
				if (warning !== null) ctx.ui.notify(warning, "warning");
			})
			.catch(() => {});
	});
}
