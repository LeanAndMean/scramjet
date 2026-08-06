import { createHash } from "node:crypto";
import { StringEnum } from "@leanandmean/ai";
import type {
	AgentToolResult,
	ExecResult,
	ExtensionAPI,
	SessionEntry,
	Theme,
	ToolRenderResultOptions,
} from "@leanandmean/coding-agent";
import { truncateHead } from "@leanandmean/coding-agent";
import { Text } from "@leanandmean/tui";
import { type Static, Type } from "typebox";
import type { ScramjetLogger } from "../logger.js";
import {
	FORGE_AUTH_FAILURE_PATTERNS,
	FORGE_EXEC_TIMEOUT_MS,
	ForgeCommandError,
	type ForgeExec,
	resolveCurrentRepository,
	UnsupportedForgeOriginError,
	withForgeMutationQueue,
} from "./client.js";
import {
	applyExactEdits,
	controlSafeText,
	type ForgeRangeRequest,
	isForgeReadDetails,
	renderForgeDocument,
	sliceForgeDocument,
} from "./document.js";
import { createGithubAdapter } from "./github.js";
import { createGitlabAdapter } from "./gitlab.js";
import type {
	ForgeAdapter,
	ForgeArtifact,
	ForgeArtifactKind,
	ForgeCreateInput,
	ForgeFieldCoverage,
	ForgeIdentity,
	ForgeMutationTarget,
	ForgePrSection,
	ForgeReadDetails,
	ForgeRepository,
} from "./types.js";

const ARTIFACT_NUMBER = Type.Integer({ minimum: 1 });
const OFFSET = Type.Optional(Type.Integer({ minimum: 1 }));
const LIMIT = Type.Optional(Type.Integer({ minimum: 1 }));
const SNAPSHOT = Type.Optional(Type.String({ pattern: "^[a-f0-9]{64}$" }));

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
			}),
		),
		offset: OFFSET,
		limit: LIMIT,
		snapshot: SNAPSHOT,
	},
	{ additionalProperties: false },
);

const TITLE = Type.String({ minLength: 1 });
const BODY = Type.String();
const COMMENT_BODY = Type.String({ minLength: 1 });
const BRANCH = Type.String({ minLength: 1 });

export const CREATE_ISSUE_SCHEMA = Type.Object({ title: TITLE, body: BODY }, { additionalProperties: false });
export const CREATE_PR_SCHEMA = Type.Object(
	{
		title: TITLE,
		body: BODY,
		head: BRANCH,
		base: BRANCH,
		draft: Type.Optional(Type.Boolean()),
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

const REPLACEMENT = {
	oldText: Type.String({ minLength: 1 }),
	newText: Type.String(),
};

function editSchema() {
	return Type.Object(
		{
			number: ARTIFACT_NUMBER,
			target: Type.Union([
				Type.Object({ kind: Type.Literal("artifact") }, { additionalProperties: false }),
				Type.Object(
					{ kind: Type.Literal("comment"), id: Type.String({ minLength: 1 }) },
					{ additionalProperties: false },
				),
			]),
			edits: Type.Array(
				Type.Object(
					{
						field: StringEnum(["title", "body"] as const),
						...REPLACEMENT,
					},
					{ additionalProperties: false },
				),
				{ minItems: 1 },
			),
		},
		{ additionalProperties: false },
	);
}

export const EDIT_ISSUE_SCHEMA = editSchema();
export const EDIT_PR_SCHEMA = editSchema();

type ReadIssueParams = Static<typeof READ_ISSUE_SCHEMA>;
type ReadPrParams = Static<typeof READ_PR_SCHEMA>;
type CreateIssueParams = Static<typeof CREATE_ISSUE_SCHEMA>;
type CreatePrParams = Static<typeof CREATE_PR_SCHEMA>;
type AddCommentParams = Static<typeof ADD_ISSUE_COMMENT_SCHEMA>;
interface EditParams {
	number: number;
	target: ForgeMutationTarget;
	edits: Array<{ field: "title" | "body"; oldText: string; newText: string }>;
}

type ForgeAdapterFactory = (repository: ForgeRepository, exec: ForgeExec, cwd: string) => ForgeAdapter;

export interface ForgeToolDependencies {
	resolveRepository?: typeof resolveCurrentRepository;
	createAdapter?: ForgeAdapterFactory;
	logger?: Pick<ScramjetLogger, "warn">;
}

function defaultAdapter(repository: ForgeRepository, exec: ForgeExec, cwd: string): ForgeAdapter {
	return repository.forge === "github" ? createGithubAdapter(exec, cwd) : createGitlabAdapter(exec, cwd);
}

function forgeExec(pi: ExtensionAPI): ForgeExec {
	return (command, args, options) => pi.exec(command, args, options);
}

function readDescription(kind: ForgeArtifactKind): string {
	const label = kind === "issue" ? "issue" : "pull request";
	return `Read a current-repository ${label} and complete top-level conversation as bounded tagged text. Continue truncated output with offset and snapshot.`;
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

function priorReadDetails(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): ForgeReadDetails[] {
	let compactionIndex = -1;
	for (let index = 0; index < entries.length; index++) {
		if (entries[index].type === "compaction") compactionIndex = index;
	}
	const currentIndexes = entries
		.map((entry, index) => (assistantHasToolCall(entry, toolCallId) ? index : -1))
		.filter((index) => index > compactionIndex);
	if (currentIndexes.length !== 1) return [];
	const currentIndex = currentIndexes[0];
	const readTool = kind === "issue" ? "read_issue" : "read_pr";
	const readCallIds = new Set<string>();
	const details: ForgeReadDetails[] = [];

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
		const receipt = message.details;
		if (
			repositoriesEqual(receipt.repository, repository) &&
			receipt.artifact.kind === kind &&
			receipt.artifact.number === number
		) {
			details.push(receipt);
		}
	}
	return details;
}

function hasCompleteParentEvidence(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): boolean {
	const groups = new Map<string, { total: number; ranges: ForgeReadDetails["core"]["ranges"]; valid: boolean }>();
	for (const details of priorReadDetails(entries, toolCallId, repository, kind, number)) {
		const group = groups.get(details.snapshot) ?? { total: details.core.totalLines, ranges: [], valid: true };
		if (group.total !== details.core.totalLines) group.valid = false;
		group.ranges.push(...details.core.ranges);
		groups.set(details.snapshot, group);
	}
	return [...groups.values()].some((group) => group.valid && completeCoverage(group.total, group.ranges));
}

function targetsEqual(left: ForgeMutationTarget, right: ForgeMutationTarget): boolean {
	if (left.kind === "artifact") return right.kind === "artifact";
	return right.kind === "comment" && left.id === right.id;
}

function hasCompleteFieldEvidence(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	target: ForgeMutationTarget,
	fields: ReadonlySet<"title" | "body">,
): boolean {
	type Coverage = { total: number; ranges: ForgeFieldCoverage["ranges"]; valid: boolean };
	const snapshots = new Map<string, Map<"title" | "body", Coverage>>();
	for (const details of priorReadDetails(entries, toolCallId, repository, kind, number)) {
		const snapshot = snapshots.get(details.snapshot) ?? new Map<"title" | "body", Coverage>();
		for (const field of details.fields) {
			if (!fields.has(field.field) || !targetsEqual(field.target, target)) continue;
			const coverage = snapshot.get(field.field) ?? {
				total: field.totalCodeUnits,
				ranges: [],
				valid: true,
			};
			if (coverage.total !== field.totalCodeUnits) coverage.valid = false;
			coverage.ranges.push(...field.ranges);
			snapshot.set(field.field, coverage);
		}
		snapshots.set(details.snapshot, snapshot);
	}
	return [...snapshots.values()].some((snapshot) =>
		[...fields].every((field) => {
			const coverage = snapshot.get(field);
			return coverage?.valid === true && completeCoverage(coverage.total, coverage.ranges);
		}),
	);
}

function assertArtifactIdentity(artifact: ForgeArtifact, kind: ForgeArtifactKind, number: number, url?: string): void {
	if (artifact.kind !== kind || artifact.number !== number || (url !== undefined && artifact.url !== url)) {
		throw new Error(`Forge adapter returned the wrong artifact for ${kind} #${number}`);
	}
}

const GITLAB_DRAFT_PREFIX = /^(?:Draft:|WIP:|\[(?:Draft|WIP)\]|\((?:Draft|WIP)\))\s*/i;

function gitlabDraftTitle(title: string): boolean {
	return GITLAB_DRAFT_PREFIX.test(title);
}

function validateCreateInput(repository: ForgeRepository, input: ForgeCreateInput): void {
	if (repository.forge !== "gitlab" || input.kind !== "pr") return;
	const prefixed = gitlabDraftTitle(input.title);
	if (!input.draft && prefixed) {
		throw new Error("GitLab cannot create a non-draft merge request with a draft-prefixed title");
	}
	if (input.draft && !prefixed) {
		throw new Error("GitLab draft merge requests require the exact approved title to include a draft prefix");
	}
}

function verifyCreatedArtifact(_repository: ForgeRepository, input: ForgeCreateInput, artifact: ForgeArtifact): void {
	if (artifact.title !== input.title || artifact.body !== input.body) {
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

type ForgeMutationOperation =
	| "create_issue"
	| "create_pr"
	| "add_issue_comment"
	| "add_pr_comment"
	| "edit_issue"
	| "edit_pr";

interface ForgeMutationSummary {
	target?: ForgeMutationTarget;
	fields?: Array<"title" | "body">;
	replacements?: number;
}

function mutationResult(
	operation: ForgeMutationOperation,
	repository: ForgeRepository,
	identity: ForgeIdentity,
	summary: ForgeMutationSummary = {},
) {
	return {
		content: [{ type: "text" as const, text: identity.url }],
		details: {
			schema: "scramjet:forge-mutation@1" as const,
			operation,
			repository,
			identity,
			...summary,
			verified: true as const,
		},
	};
}

type ForgeFailureClass =
	| "FORGE_READ_FAILED"
	| "FORGE_PREFLIGHT_FAILED"
	| "FORGE_WRITE_REJECTED"
	| "FORGE_WRITE_AMBIGUOUS";

type ForgeFailurePhase = "repository" | "evidence" | "refetch" | "dispatch" | "response" | "verify";
type ForgeWriteState = "not_attempted" | "rejected" | "possible";

interface ForgeFailureDetails {
	schema: "scramjet:forge-failure@1";
	class: ForgeFailureClass;
	operation: string;
	phase: ForgeFailurePhase;
	writeState: ForgeWriteState;
	diagnostic?: ForgeCommandError["invocation"];
	trace?: string;
}

class ForgePreflightError extends Error {
	readonly phase: "refetch";

	constructor(cause: Error) {
		super("Forge mutation preflight refetch failed", { cause });
		this.name = "ForgePreflightError";
		this.phase = "refetch";
	}
}

class ForgeMutationAmbiguityError extends Error {
	readonly phase: "dispatch" | "response" | "verify";

	constructor(operation: string, phase: "dispatch" | "response" | "verify", cause: Error) {
		super(`${operation} may have succeeded`, { cause });
		this.name = "ForgeMutationAmbiguityError";
		this.phase = phase;
	}
}

function forgeCommandError(error: unknown): ForgeCommandError | undefined {
	let current = error;
	for (let depth = 0; depth < 5 && current instanceof Error; depth++) {
		if (current instanceof ForgeCommandError) return current;
		current = current.cause;
	}
	return undefined;
}

function ambiguousMutation(
	operation: string,
	error: unknown,
	phase: "dispatch" | "response" | "verify" = "verify",
): Error {
	return new ForgeMutationAmbiguityError(operation, phase, error instanceof Error ? error : new Error(String(error)));
}

function mutationAttemptFailure(operation: string, error: unknown): Error {
	if (error instanceof ForgeCommandError && error.kind === "missing-executable") return error;
	return ambiguousMutation(operation, error, error instanceof ForgeCommandError ? "dispatch" : "response");
}

function deepestError(error: unknown): Error | undefined {
	let current = error instanceof Error ? error : undefined;
	for (let depth = 0; depth < 5 && current?.cause instanceof Error; depth++) current = current.cause;
	return current;
}

function boundedFailureCause(error: unknown, commandError: ForgeCommandError | undefined): string {
	const cause = commandError ?? deepestError(error);
	const value = controlSafeText(cause?.message ?? String(error));
	const truncated = truncateHead(value, { maxBytes: 240, maxLines: 4 });
	if (!truncated.truncated) return truncated.content;
	const marker = `[truncated ${truncated.totalBytes} bytes/${truncated.totalLines} lines; sha256 ${createHash("sha256").update(value, "utf8").digest("hex")}]`;
	return truncated.content === "" ? marker : `${truncated.content}\n${marker}`;
}

function failureTrace(error: unknown): string | undefined {
	const cause = deepestError(error);
	if (cause?.stack === undefined) return undefined;
	const frames = controlSafeText(cause.stack.split("\n").slice(1, 7).join("\n"));
	const truncated = truncateHead(frames, { maxBytes: 1024, maxLines: 6 });
	return truncated.content === "" ? undefined : truncated.content;
}

function failureResult(
	operation: string,
	mode: "read" | "mutation",
	error: unknown,
): AgentToolResult<ForgeFailureDetails> {
	const commandError = forgeCommandError(error);
	const ambiguity = error instanceof ForgeMutationAmbiguityError ? error : undefined;
	const preflight = error instanceof ForgePreflightError ? error : undefined;
	const unsupportedOrigin = error instanceof UnsupportedForgeOriginError;
	const failureClass: ForgeFailureClass =
		mode === "read"
			? "FORGE_READ_FAILED"
			: ambiguity
				? "FORGE_WRITE_AMBIGUOUS"
				: commandError?.kind === "missing-executable"
					? "FORGE_WRITE_REJECTED"
					: "FORGE_PREFLIGHT_FAILED";
	const phase: ForgeFailurePhase =
		ambiguity?.phase ??
		preflight?.phase ??
		(unsupportedOrigin || commandError?.invocation.command === "git"
			? "repository"
			: mode === "read"
				? "response"
				: commandError?.kind === "missing-executable"
					? "dispatch"
					: "evidence");
	const writeState: ForgeWriteState =
		failureClass === "FORGE_WRITE_AMBIGUOUS"
			? "possible"
			: failureClass === "FORGE_WRITE_REJECTED"
				? "rejected"
				: "not_attempted";
	const cli = commandError?.invocation.command;
	const providerCli = cli === "gh" || cli === "glab" ? cli : undefined;
	const cause = boundedFailureCause(error, commandError);
	const trace = failureTrace(error);
	const recovery = unsupportedOrigin
		? "No write was attempted. The current origin is not a supported public GitHub or GitLab repository, so no provider CLI fallback was selected. Correct the origin or report this Scramjet failure code."
		: failureClass === "FORGE_READ_FAILED"
			? providerCli === undefined
				? "No write was attempted. No provider CLI fallback was selected. Correct repository resolution or report this Scramjet failure code."
				: `No write was attempted. If bash is available, deliberate read-only ${providerCli} inspection is a fallback. If direct CLI inspection succeeds, report this Scramjet failure code.`
			: failureClass === "FORGE_PREFLIGHT_FAILED"
				? "No write was attempted. Correct the repository, evidence, input, or provider constraint, then make a new forge-tool call; do not substitute a CLI mutation. If the cause is not actionable, report this Scramjet failure code."
				: failureClass === "FORGE_WRITE_REJECTED"
					? "No request reached the provider. Correct the prerequisite, then make a new deliberate forge-tool call; do not substitute a CLI mutation."
					: `The mutation may have succeeded. DO NOT RETRY this call or run a CLI mutation fallback. Reconcile with a fresh forge read${providerCli === undefined ? "." : ` or, when bash is available, read-only ${providerCli} inspection.`}`;
	return {
		content: [
			{
				type: "text",
				text: `${failureClass} operation=${operation} phase=${phase} write=${writeState}\n${cause}\n${recovery}`,
			},
		],
		details: {
			schema: "scramjet:forge-failure@1",
			class: failureClass,
			operation,
			phase,
			writeState,
			...(commandError === undefined ? {} : { diagnostic: commandError.invocation }),
			...(trace === undefined ? {} : { trace }),
		},
		isError: true,
	};
}

async function executeForgeTool<T>(
	operation: string,
	mode: "read" | "mutation",
	execute: () => Promise<AgentToolResult<T>>,
): Promise<AgentToolResult<T | ForgeFailureDetails>> {
	try {
		return await execute();
	} catch (error) {
		return failureResult(operation, mode, error);
	}
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
		return mutationResult(kind === "issue" ? "create_issue" : "create_pr", repository, identity);
	} catch (error) {
		throw ambiguousMutation(operation, error);
	}
}

function mutationQueueKey(
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	target: ForgeMutationTarget,
): string {
	return JSON.stringify([
		repository.forge,
		repository.host,
		repository.projectPath,
		kind,
		number,
		target.kind === "artifact" ? target.kind : [target.kind, target.id],
	]);
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
	const key = mutationQueueKey(repository, kind, params.number, { kind: "artifact" });
	return withForgeMutationQueue(key, async () => {
		let parent: ForgeArtifact;
		try {
			parent = await adapter.readArtifact(repository, kind, params.number, [], signal);
			assertArtifactIdentity(parent, kind, params.number);
		} catch (error) {
			throw new ForgePreflightError(error instanceof Error ? error : new Error(String(error)));
		}
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
			return mutationResult(kind === "issue" ? "add_issue_comment" : "add_pr_comment", repository, identity);
		} catch (error) {
			throw ambiguousMutation(operation, error);
		}
	});
}

function editMutationResult(
	operation: "edit_issue" | "edit_pr",
	repository: ForgeRepository,
	identity: ForgeIdentity,
	target: ForgeMutationTarget,
	edits: EditParams["edits"],
) {
	return mutationResult(operation, repository, identity, {
		target,
		fields: [...new Set(edits.map((edit) => edit.field))],
		replacements: edits.length,
	});
}

async function editArtifact(
	kind: ForgeArtifactKind,
	params: EditParams,
	toolCallId: string,
	signal: AbortSignal | undefined,
	cwd: string,
	entries: readonly SessionEntry[],
	exec: ForgeExec,
	dependencies: Required<ForgeToolDependencies>,
) {
	const target = params.target;
	if (target.kind === "comment" && params.edits.some((edit) => edit.field !== "body")) {
		throw new Error("Comment edits may target only the body field");
	}
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	const fields = new Set<"title" | "body">(params.edits.map((edit) => edit.field));
	if (!hasCompleteFieldEvidence(entries, toolCallId, repository, kind, params.number, target, fields)) {
		const operation = kind === "issue" ? "edit_issue" : "edit_pr";
		const readTool = kind === "issue" ? "read_issue" : "read_pr";
		throw new Error(
			`${operation} requires a complete prior ${readTool} of ${kind} #${params.number} on the active branch after the latest compaction for every edited field (${[...fields].join(", ")}), using all ranges from one unchanged snapshot`,
		);
	}
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const key = mutationQueueKey(repository, kind, params.number, target);
	return withForgeMutationQueue(key, async () => {
		let original: ForgeArtifact;
		try {
			original = await adapter.readArtifact(repository, kind, params.number, [], signal);
			assertArtifactIdentity(original, kind, params.number);
		} catch (error) {
			throw new ForgePreflightError(error instanceof Error ? error : new Error(String(error)));
		}
		const operation = kind === "issue" ? "Issue edit" : "Pull request edit";
		const toolName = kind === "issue" ? "edit_issue" : "edit_pr";

		if (target.kind === "artifact") {
			const titleEdits = params.edits
				.filter((edit) => edit.field === "title")
				.map(({ oldText, newText }) => ({ oldText, newText }));
			const bodyEdits = params.edits
				.filter((edit) => edit.field === "body")
				.map(({ oldText, newText }) => ({ oldText, newText }));
			const title = titleEdits.length === 0 ? original.title : applyExactEdits(original.title, titleEdits, "title");
			const body = bodyEdits.length === 0 ? original.body : applyExactEdits(original.body, bodyEdits, "body");
			if (
				repository.forge === "gitlab" &&
				kind === "pr" &&
				original.kind === "pr" &&
				titleEdits.length > 0 &&
				gitlabDraftTitle(title) !== original.readiness.draft
			) {
				throw new Error("GitLab pull request title edits must preserve the existing draft state");
			}

			let identity: Awaited<ReturnType<ForgeAdapter["updateArtifact"]>>;
			try {
				identity = await adapter.updateArtifact(
					repository,
					{
						kind,
						number: params.number,
						...(titleEdits.length === 0 ? {} : { title }),
						...(bodyEdits.length === 0 ? {} : { body }),
					},
					signal,
				);
				if (identity.kind !== kind || identity.number !== params.number || identity.url !== original.url) {
					throw new Error("Forge adapter returned the wrong updated artifact identity");
				}
			} catch (error) {
				throw mutationAttemptFailure(operation, error);
			}
			try {
				const updated = await adapter.readArtifact(repository, kind, params.number, [], signal);
				assertArtifactIdentity(updated, kind, params.number, original.url);
				if (updated.title !== title || updated.body !== body) {
					throw new Error("Updated artifact mutable content did not match the requested postimage");
				}
				if (
					repository.forge === "gitlab" &&
					kind === "pr" &&
					original.kind === "pr" &&
					(updated.kind !== "pr" || updated.readiness.draft !== original.readiness.draft)
				) {
					throw new Error("Updated GitLab pull request changed draft state");
				}
				return editMutationResult(toolName, repository, identity, target, params.edits);
			} catch (error) {
				throw ambiguousMutation(operation, error);
			}
		}

		const comment = original.comments.find((candidate) => candidate.id === target.id);
		if (comment === undefined) throw new Error(`Comment ${target.id} was not found in ${kind} #${params.number}`);
		const body = applyExactEdits(
			comment.body,
			params.edits.map(({ oldText, newText }) => ({ oldText, newText })),
			"comment body",
		);
		let identity: Awaited<ReturnType<ForgeAdapter["updateComment"]>>;
		try {
			identity = await adapter.updateComment(
				repository,
				{ kind, number: params.number, id: target.id, body },
				signal,
			);
			if (identity.id !== target.id || identity.url !== comment.url) {
				throw new Error("Forge adapter returned the wrong updated comment identity");
			}
		} catch (error) {
			throw mutationAttemptFailure(operation, error);
		}
		try {
			const updated = await adapter.readArtifact(repository, kind, params.number, [], signal);
			assertArtifactIdentity(updated, kind, params.number, original.url);
			const updatedComment = updated.comments.find((candidate) => candidate.id === target.id);
			if (updatedComment === undefined || updatedComment.url !== comment.url || updatedComment.body !== body) {
				throw new Error("Updated comment mutable content did not match the requested postimage");
			}
			return editMutationResult(toolName, repository, identity, target, params.edits);
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
	let content = theme.fg("success", `positions ${offset}-${end} of ${totalLines}`);
	if (options.expanded) {
		const output = resultText(result);
		if (output !== "") content += `\n${theme.fg("toolOutput", output)}`;
	}
	text.setText(content);
	return text;
}

function prerequisiteWarning(repository: ForgeRepository, result: ExecResult): string | null {
	const cli = repository.forge === "github" ? "gh" : "glab";
	const name = repository.forge === "github" ? "GitHub CLI" : "GitLab CLI";
	if (result.spawnError?.code === "ENOENT") {
		return `Scramjet forge tools require ${name} (${cli}), but it is not installed.`;
	}
	if (result.code === 0 || result.killed || result.spawnError || result.stdinError) return null;
	const diagnostic = `${result.stdout}\n${result.stderr}`;
	if (!FORGE_AUTH_FAILURE_PATTERNS.some((pattern) => pattern.test(diagnostic))) return null;
	return `Scramjet forge tools cannot authenticate to ${repository.host}. Run \`${cli} auth login --hostname ${repository.host}\`, then restart Scramjet.`;
}

async function probePrerequisite(
	exec: ForgeExec,
	cwd: string,
	resolveRepository: typeof resolveCurrentRepository,
): Promise<string | null> {
	let repository: ForgeRepository;
	try {
		repository = await resolveRepository(exec, cwd);
	} catch (error) {
		if (error instanceof ForgeCommandError || error instanceof UnsupportedForgeOriginError) return null;
		throw error;
	}
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
		logger: overrides.logger ?? { warn() {} },
	};
	const exec = forgeExec(pi);

	pi.registerTool({
		name: "read_issue",
		label: "read issue",
		description: readDescription("issue"),
		promptSnippet: "Read a current-repository issue conversation",
		promptGuidelines: ["Continue truncated read_issue output with offset and the unchanged snapshot until complete."],
		parameters: READ_ISSUE_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("read_issue", "read", () =>
				readArtifact("issue", params, signal, ctx.cwd, exec, dependencies),
			);
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
		promptSnippet: "Read a current-repository pull request conversation",
		promptGuidelines: ["Continue truncated read_pr output with offset and the unchanged snapshot until complete."],
		parameters: READ_PR_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("read_pr", "read", () =>
				readArtifact("pr", params, signal, ctx.cwd, exec, dependencies),
			);
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
		promptSnippet: "Create a current-repository issue",
		promptGuidelines: ["Call create_issue only after exact title and body approval."],
		parameters: CREATE_ISSUE_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("create_issue", "mutation", () =>
				createArtifact("issue", params, signal, ctx.cwd, exec, dependencies),
			);
		},
	});

	pi.registerTool({
		name: "create_pr",
		label: "create pull request",
		description:
			"Create and verify a pull request in the current repository with exact content and explicit head/base branches. This never creates, checks out, or pushes branches.",
		promptSnippet: "Create a PR from explicit existing branches",
		promptGuidelines: [
			"Call create_pr only after exact title and body approval; GitLab draft titles must already carry a draft prefix.",
			"Pass explicit head/base branches; create_pr never mutates Git state.",
		],
		parameters: CREATE_PR_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("create_pr", "mutation", () =>
				createArtifact("pr", params, signal, ctx.cwd, exec, dependencies),
			);
		},
	});

	pi.registerTool({
		name: "add_issue_comment",
		label: "add issue comment",
		description:
			"Add and verify one top-level issue comment after a complete prior read_issue of the parent conversation.",
		promptSnippet: "Add a comment to a previously read issue",
		promptGuidelines: ["Before add_issue_comment, completely read the parent with read_issue."],
		parameters: ADD_ISSUE_COMMENT_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("add_issue_comment", "mutation", () =>
				addComment(
					"issue",
					params,
					toolCallId,
					signal,
					ctx.cwd,
					ctx.sessionManager.getBranch(),
					exec,
					dependencies,
				),
			);
		},
	});

	pi.registerTool({
		name: "add_pr_comment",
		label: "add pull request comment",
		description:
			"Add and verify one top-level pull request comment after a complete prior read_pr of the parent conversation.",
		promptSnippet: "Add a comment to a previously read PR",
		promptGuidelines: ["Before add_pr_comment, completely read the parent with read_pr."],
		parameters: ADD_PR_COMMENT_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("add_pr_comment", "mutation", () =>
				addComment("pr", params, toolCallId, signal, ctx.cwd, ctx.sessionManager.getBranch(), exec, dependencies),
			);
		},
	});

	pi.registerTool({
		name: "edit_issue",
		label: "edit issue",
		description:
			"Edit one previously read issue or top-level issue comment using exact, unique, non-overlapping replacements against the current decoded content.",
		promptSnippet: "Exactly edit a previously read issue or comment",
		promptGuidelines: [
			"Before edit_issue, completely read every edited field; replacements match decoded text exactly and target one object.",
		],
		parameters: EDIT_ISSUE_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("edit_issue", "mutation", () =>
				editArtifact(
					"issue",
					params,
					toolCallId,
					signal,
					ctx.cwd,
					ctx.sessionManager.getBranch(),
					exec,
					dependencies,
				),
			);
		},
	});

	pi.registerTool({
		name: "edit_pr",
		label: "edit pull request",
		description:
			"Edit one previously read pull request or top-level pull request comment using exact, unique, non-overlapping replacements against the current decoded content.",
		promptSnippet: "Exactly edit a previously read PR or comment",
		promptGuidelines: [
			"Before edit_pr, completely read every edited field; replacements match decoded text exactly and target one object.",
		],
		parameters: EDIT_PR_SCHEMA,
		async execute(toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("edit_pr", "mutation", () =>
				editArtifact("pr", params, toolCallId, signal, ctx.cwd, ctx.sessionManager.getBranch(), exec, dependencies),
			);
		},
	});

	pi.on("session_start", (_event, ctx) => {
		if (!ctx.hasUI) return;
		void probePrerequisite(exec, ctx.cwd, dependencies.resolveRepository)
			.then((warning) => {
				if (warning !== null) ctx.ui.notify(warning, "warning");
			})
			.catch((error) => {
				dependencies.logger.warn("forge", "startup prerequisite probe failed", { error: String(error) });
			});
	});
}
