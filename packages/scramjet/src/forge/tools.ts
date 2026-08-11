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
import { createGithubAdapter } from "./github.js";
import { createGitlabAdapter } from "./gitlab.js";
import {
	executeForgeReadPlan,
	hasCompleteSegmentCoverage,
	isForgeReadDetails,
	isForgeReadPayload,
	type PreparedForgeRead,
	windowForgeRead,
} from "./native-reply.js";
import { prettyForgeReply, rawForgeReply } from "./renderer.js";
import { applyExactEdits, controlSafeText } from "./text.js";
import type {
	ForgeAdapter,
	ForgeArtifactKind,
	ForgeCreateInput,
	ForgeEditable,
	ForgeIdentity,
	ForgeMutationTarget,
	ForgeReadSegmentId,
	ForgeReadSegmentReceipt,
	ForgeRepository,
} from "./types.js";

const ARTIFACT_NUMBER = Type.Integer({ minimum: 1 });
const OFFSET = Type.Optional(Type.Integer({ minimum: 1 }));
const LIMIT = Type.Optional(Type.Integer({ minimum: 1 }));
const BYTE_OFFSET = Type.Optional(Type.Integer({ minimum: 1 }));
const SNAPSHOT = Type.Optional(Type.String({ pattern: "^[a-f0-9]{64}$" }));
const READ_SEGMENTS = [
	"artifact",
	"comments",
	"sub_issues",
	"parent",
	"relationships",
	"files",
	"commits",
	"check_runs",
	"status",
	"pipelines",
] as const;
const INCLUDE = Type.Optional(Type.Array(StringEnum(READ_SEGMENTS), { uniqueItems: true, minItems: 1, maxItems: 10 }));

function readSchema() {
	return Type.Object(
		{
			number: ARTIFACT_NUMBER,
			include: INCLUDE,
			offset: OFFSET,
			limit: LIMIT,
			byte_offset: BYTE_OFFSET,
			snapshot: SNAPSHOT,
		},
		{ additionalProperties: false },
	);
}

export const READ_ISSUE_SCHEMA = readSchema();
export const READ_PR_SCHEMA = readSchema();

const TITLE = Type.String({ minLength: 1 });
const BODY = Type.String();
const COMMENT_BODY = Type.String({ minLength: 1 });
const BRANCH = Type.String({ minLength: 1 });

export const CREATE_ISSUE_SCHEMA = Type.Object({ title: TITLE, body: BODY }, { additionalProperties: false });
export const CREATE_PR_SCHEMA = Type.Object(
	{ title: TITLE, body: BODY, head: BRANCH, base: BRANCH, draft: Type.Optional(Type.Boolean()) },
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
						oldText: Type.String({ minLength: 1 }),
						newText: Type.String(),
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

type ReadParams = Static<typeof READ_ISSUE_SCHEMA>;
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

function defaultSegments(repository: ForgeRepository, kind: ForgeArtifactKind): ForgeReadSegmentId[] {
	if (kind === "pr") return ["artifact", "comments"];
	return repository.forge === "github"
		? ["artifact", "comments", "sub_issues", "parent"]
		: ["artifact", "comments", "relationships"];
}

function readDescription(kind: ForgeArtifactKind): string {
	const label = kind === "issue" ? "issue" : "pull request";
	return `Read a current-repository ${label} as filtered native gh/glab JSON command replies. Omit include for the standard first read; select exact segments for subset rereads. Continue item windows with the returned include, offset, and snapshot.`;
}

function repositoriesEqual(left: ForgeRepository, right: ForgeRepository): boolean {
	return left.forge === right.forge && left.host === right.host && left.projectPath === right.projectPath;
}

function sameArtifact(
	read: PreparedForgeRead,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): boolean {
	return (
		repositoriesEqual(read.repository, repository) && read.artifact.kind === kind && read.artifact.number === number
	);
}

function cacheRead(cache: Map<string, PreparedForgeRead>, read: PreparedForgeRead): void {
	cache.set(read.snapshot, read);
	while (cache.size > 8) cache.delete(cache.keys().next().value as string);
}

function invalidateCache(
	cache: Map<string, PreparedForgeRead>,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	roles: readonly ("artifact" | "comments")[],
): void {
	for (const read of cache.values()) {
		if (!sameArtifact(read, repository, kind, number)) continue;
		read.segments = read.segments.filter(
			(segment) => segment.spec.evidence === undefined || !roles.includes(segment.spec.evidence),
		);
	}
}

async function readArtifact(
	kind: ForgeArtifactKind,
	params: ReadParams,
	signal: AbortSignal | undefined,
	cwd: string,
	exec: ForgeExec,
	dependencies: Required<ForgeToolDependencies>,
	cache: Map<string, PreparedForgeRead>,
) {
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	let read: PreparedForgeRead;
	let include: ForgeReadSegmentId[];
	if (params.snapshot === undefined) {
		if ((params.offset !== undefined && params.offset !== 1) || params.byte_offset !== undefined) {
			throw new Error("Start a fresh forge read at offset=1 without byte_offset");
		}
		include = [...((params.include as ForgeReadSegmentId[] | undefined) ?? defaultSegments(repository, kind))];
		const adapter = dependencies.createAdapter(repository, exec, cwd);
		const plan = adapter.readPlan(repository, kind, params.number, include);
		read = await executeForgeReadPlan(plan, exec, cwd, signal);
		cacheRead(cache, read);
	} else {
		const cached = cache.get(params.snapshot);
		if (cached === undefined || !sameArtifact(cached, repository, kind, params.number)) {
			throw new Error("Forge continuation snapshot is unavailable; restart without snapshot");
		}
		read = cached;
		include = [...((params.include as ForgeReadSegmentId[] | undefined) ?? cached.include)];
	}
	const window = windowForgeRead(read, {
		include,
		...(params.offset === undefined ? {} : { offset: params.offset }),
		...(params.limit === undefined ? {} : { limit: params.limit }),
		...(params.byte_offset === undefined ? {} : { byteOffset: params.byte_offset }),
	});
	return { content: [{ type: "text" as const, text: window.content }], details: window.details };
}

function assistantHasToolCall(entry: SessionEntry, toolCallId: string): boolean {
	return (
		entry.type === "message" &&
		entry.message.role === "assistant" &&
		entry.message.content.some((block) => block.type === "toolCall" && block.id === toolCallId)
	);
}

interface MutationEvidenceDetails {
	schema: "scramjet:forge-mutation@1";
	repository: ForgeRepository;
	artifact: { kind: ForgeArtifactKind; number: number };
	invalidates: Array<"artifact" | "comments">;
}

interface FailureEvidenceDetails {
	schema: "scramjet:forge-failure@1";
	repository?: ForgeRepository;
	artifact?: { kind: ForgeArtifactKind; number: number };
	invalidates?: Array<"artifact" | "comments">;
}

function validEvidenceRepository(value: unknown): value is ForgeRepository {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const repository = value as Record<string, unknown>;
	return (
		(repository.forge === "github" || repository.forge === "gitlab") &&
		(repository.host === "github.com" || repository.host === "gitlab.com") &&
		typeof repository.projectPath === "string" &&
		repository.projectPath !== "" &&
		(repository.forge === "github") === (repository.host === "github.com")
	);
}

function validEvidenceArtifact(value: unknown): value is { kind: ForgeArtifactKind; number: number } {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const artifact = value as Record<string, unknown>;
	return (
		(artifact.kind === "issue" || artifact.kind === "pr") &&
		Number.isInteger(artifact.number) &&
		(artifact.number as number) > 0
	);
}

function mutationEvidenceDetails(value: unknown): MutationEvidenceDetails | FailureEvidenceDetails | null {
	if (typeof value !== "object" || value === null) return null;
	const details = value as Record<string, unknown>;
	if (details.schema !== "scramjet:forge-mutation@1" && details.schema !== "scramjet:forge-failure@1") return null;
	if (
		!Array.isArray(details.invalidates) ||
		details.invalidates.some((role: unknown) => role !== "artifact" && role !== "comments") ||
		(details.repository !== undefined && !validEvidenceRepository(details.repository)) ||
		(details.artifact !== undefined && !validEvidenceArtifact(details.artifact)) ||
		(details.schema === "scramjet:forge-mutation@1" &&
			(details.repository === undefined || details.artifact === undefined))
	) {
		return null;
	}
	return details as unknown as MutationEvidenceDetails | FailureEvidenceDetails;
}

function priorSegmentEvidence(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): Map<"artifact" | "comments", Map<string, ForgeReadSegmentReceipt[]>> {
	let compactionIndex = -1;
	for (let index = 0; index < entries.length; index++)
		if (entries[index].type === "compaction") compactionIndex = index;
	const currentIndexes = entries
		.map((entry, index) => (assistantHasToolCall(entry, toolCallId) ? index : -1))
		.filter((index) => index > compactionIndex);
	const evidence = new Map<"artifact" | "comments", Map<string, ForgeReadSegmentReceipt[]>>();
	if (currentIndexes.length !== 1) return evidence;
	const currentIndex = currentIndexes[0];
	const readTool = kind === "issue" ? "read_issue" : "read_pr";
	const readCallIds = new Set<string>();

	for (let index = compactionIndex + 1; index < currentIndex; index++) {
		const entry = entries[index];
		if (entry.type !== "message") continue;
		const message = entry.message;
		if (message.role === "assistant") {
			for (const block of message.content)
				if (block.type === "toolCall" && block.name === readTool) readCallIds.add(block.id);
			continue;
		}
		if (message.role !== "toolResult") continue;
		const mutation = mutationEvidenceDetails(message.details);
		if (
			mutation !== null &&
			mutation.repository !== undefined &&
			mutation.artifact !== undefined &&
			repositoriesEqual(mutation.repository, repository) &&
			mutation.artifact.kind === kind &&
			mutation.artifact.number === number
		) {
			for (const role of mutation.invalidates ?? []) evidence.delete(role);
		}
		const persistedText = message.content.find((part) => part.type === "text")?.text;
		if (
			message.isError ||
			message.toolName !== readTool ||
			!readCallIds.has(message.toolCallId) ||
			typeof persistedText !== "string" ||
			!isForgeReadPayload(persistedText, message.details)
		) {
			continue;
		}
		const receipt = message.details;
		if (
			!repositoriesEqual(receipt.repository, repository) ||
			receipt.artifact.kind !== kind ||
			receipt.artifact.number !== number
		) {
			continue;
		}
		for (const segment of receipt.segments) {
			if (segment.status !== "ok" || segment.evidence === undefined || segment.snapshot === undefined) continue;
			const snapshots = evidence.get(segment.evidence) ?? new Map<string, ForgeReadSegmentReceipt[]>();
			const receipts = snapshots.get(segment.snapshot) ?? [];
			receipts.push(segment);
			snapshots.set(segment.snapshot, receipts);
			evidence.set(segment.evidence, snapshots);
		}
	}
	return evidence;
}

function hasCompleteEvidence(
	entries: readonly SessionEntry[],
	toolCallId: string,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	roles: readonly ("artifact" | "comments")[],
): boolean {
	const evidence = priorSegmentEvidence(entries, toolCallId, repository, kind, number);
	return roles.every((role) => {
		const snapshots = evidence.get(role);
		return snapshots !== undefined && [...snapshots.values()].some(hasCompleteSegmentCoverage);
	});
}

function assertEditableArtifact(
	editable: ForgeEditable,
	kind: ForgeArtifactKind,
	number: number,
	url?: string,
): asserts editable is Extract<ForgeEditable, { target: { kind: "artifact" } }> {
	if (
		editable.target.kind !== "artifact" ||
		editable.kind !== kind ||
		editable.number !== number ||
		(url !== undefined && editable.url !== url)
	) {
		throw new Error(`Forge adapter returned the wrong artifact for ${kind} #${number}`);
	}
}

function assertEditableComment(
	editable: ForgeEditable,
	kind: ForgeArtifactKind,
	number: number,
	id: string,
	url?: string,
): asserts editable is Extract<ForgeEditable, { target: { kind: "comment" } }> {
	if (
		editable.target.kind !== "comment" ||
		editable.target.id !== id ||
		editable.kind !== kind ||
		editable.number !== number ||
		(url !== undefined && editable.url !== url)
	) {
		throw new Error(`Forge adapter returned the wrong comment ${id} for ${kind} #${number}`);
	}
}

const GITLAB_DRAFT_PREFIX = /^(?:Draft:|WIP:|\[(?:Draft|WIP)\]|\((?:Draft|WIP)\))\s*/i;

function gitlabDraftTitle(title: string): boolean {
	return GITLAB_DRAFT_PREFIX.test(title);
}

function validateCreateInput(repository: ForgeRepository, input: ForgeCreateInput): void {
	if (repository.forge !== "gitlab" || input.kind !== "pr") return;
	const prefixed = gitlabDraftTitle(input.title);
	if (!input.draft && prefixed)
		throw new Error("GitLab cannot create a non-draft merge request with a draft-prefixed title");
	if (input.draft && !prefixed) {
		throw new Error("GitLab draft merge requests require the exact approved title to include a draft prefix");
	}
}

type ForgeMutationOperation =
	| "create_issue"
	| "create_pr"
	| "add_issue_comment"
	| "add_pr_comment"
	| "edit_issue"
	| "edit_pr";

function mutationResult(
	operation: ForgeMutationOperation,
	repository: ForgeRepository,
	artifact: { kind: ForgeArtifactKind; number: number },
	identity: ForgeIdentity,
	invalidates: Array<"artifact" | "comments">,
	summary: { target?: ForgeMutationTarget; fields?: Array<"title" | "body">; replacements?: number } = {},
) {
	return {
		content: [{ type: "text" as const, text: identity.url }],
		details: {
			schema: "scramjet:forge-mutation@1" as const,
			operation,
			repository,
			artifact,
			identity,
			invalidates,
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
interface MutationContext {
	repository: ForgeRepository;
	artifact: { kind: ForgeArtifactKind; number: number };
	invalidates: Array<"artifact" | "comments">;
}
interface ForgeFailureDetails extends FailureEvidenceDetails {
	class: ForgeFailureClass;
	operation: string;
	phase: ForgeFailurePhase;
	writeState: ForgeWriteState;
	diagnostic?: ForgeCommandError["invocation"];
	trace?: string;
}

class ForgePreflightError extends Error {
	readonly phase = "refetch" as const;
	constructor(cause: Error) {
		super("Forge mutation preflight refetch failed", { cause });
		this.name = "ForgePreflightError";
	}
}

class ForgeMutationAmbiguityError extends Error {
	constructor(
		operation: string,
		readonly phase: "dispatch" | "response" | "verify",
		cause: Error,
		readonly context?: MutationContext,
	) {
		super(`${operation} may have succeeded`, { cause });
		this.name = "ForgeMutationAmbiguityError";
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
	context?: MutationContext,
): Error {
	return new ForgeMutationAmbiguityError(
		operation,
		phase,
		error instanceof Error ? error : new Error(String(error)),
		context,
	);
}

function mutationAttemptFailure(operation: string, error: unknown, context?: MutationContext): Error {
	if (error instanceof ForgeCommandError && error.kind === "missing-executable") return error;
	return ambiguousMutation(operation, error, error instanceof ForgeCommandError ? "dispatch" : "response", context);
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
		? "No write was attempted. Correct the unsupported origin or report this Scramjet failure code."
		: failureClass === "FORGE_READ_FAILED"
			? providerCli === undefined
				? "No write was attempted. Correct repository resolution or report this Scramjet failure code."
				: `No write was attempted. If bash is available, deliberate read-only ${providerCli} inspection is a fallback.`
			: failureClass === "FORGE_PREFLIGHT_FAILED"
				? "No write was attempted. Correct the repository, evidence, input, or provider constraint; do not substitute a CLI mutation."
				: failureClass === "FORGE_WRITE_REJECTED"
					? "No request reached the provider. Correct the prerequisite, then make a new deliberate forge-tool call."
					: `The mutation may have succeeded. DO NOT RETRY or run a CLI mutation fallback. Reconcile with a fresh forge read${providerCli === undefined ? "." : ` or read-only ${providerCli} inspection.`}`;
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
			...(ambiguity?.context ?? {}),
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
	cache: Map<string, PreparedForgeRead>,
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
		const editable = await adapter.readEditable(repository, kind, identity.number, { kind: "artifact" }, signal);
		assertEditableArtifact(editable, kind, identity.number, identity.url);
		if (editable.title !== input.title || editable.body !== input.body) {
			throw new Error("Created artifact content did not match the requested title and body");
		}
		if (
			input.kind === "pr" &&
			(editable.kind !== "pr" ||
				editable.head !== input.head ||
				editable.base !== input.base ||
				editable.draft !== input.draft)
		) {
			throw new Error("Created pull request did not match the requested branches and draft state");
		}
		invalidateCache(cache, repository, kind, identity.number, ["artifact", "comments"]);
		return mutationResult(
			kind === "issue" ? "create_issue" : "create_pr",
			repository,
			{ kind, number: identity.number },
			identity,
			["artifact", "comments"],
		);
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
	cache: Map<string, PreparedForgeRead>,
) {
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	if (!hasCompleteEvidence(entries, toolCallId, repository, kind, params.number, ["artifact", "comments"])) {
		throw new Error(
			`${kind === "issue" ? "add_issue_comment" : "add_pr_comment"} requires complete prior artifact and comments segments from ${kind === "issue" ? "read_issue" : "read_pr"}`,
		);
	}
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const context: MutationContext = {
		repository,
		artifact: { kind, number: params.number },
		invalidates: ["comments"],
	};
	return withForgeMutationQueue(mutationQueueKey(repository, kind, params.number, { kind: "artifact" }), async () => {
		try {
			const parent = await adapter.readEditable(repository, kind, params.number, { kind: "artifact" }, signal);
			assertEditableArtifact(parent, kind, params.number);
		} catch (error) {
			throw new ForgePreflightError(error instanceof Error ? error : new Error(String(error)));
		}
		const operation = kind === "issue" ? "Issue comment creation" : "PR comment creation";
		invalidateCache(cache, repository, kind, params.number, ["comments"]);
		let identity: Awaited<ReturnType<ForgeAdapter["addComment"]>>;
		try {
			identity = await adapter.addComment(repository, { kind, number: params.number, body: params.body }, signal);
		} catch (error) {
			throw mutationAttemptFailure(operation, error, context);
		}
		try {
			const comment = await adapter.readEditable(
				repository,
				kind,
				params.number,
				{ kind: "comment", id: identity.id },
				signal,
			);
			assertEditableComment(comment, kind, params.number, identity.id, identity.url);
			if (comment.body !== params.body) throw new Error("Created comment did not match the requested body");
			return mutationResult(
				kind === "issue" ? "add_issue_comment" : "add_pr_comment",
				repository,
				context.artifact,
				identity,
				["comments"],
			);
		} catch (error) {
			throw ambiguousMutation(operation, error, "verify", context);
		}
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
	cache: Map<string, PreparedForgeRead>,
) {
	if (params.target.kind === "comment" && params.edits.some((edit) => edit.field !== "body")) {
		throw new Error("Comment edits may target only the body field");
	}
	const repository = await dependencies.resolveRepository(exec, cwd, signal);
	const role = params.target.kind === "artifact" ? "artifact" : "comments";
	if (!hasCompleteEvidence(entries, toolCallId, repository, kind, params.number, [role])) {
		throw new Error(
			`${kind === "issue" ? "edit_issue" : "edit_pr"} requires complete prior ${role} segment evidence on the active branch after compaction`,
		);
	}
	const adapter = dependencies.createAdapter(repository, exec, cwd);
	const context: MutationContext = { repository, artifact: { kind, number: params.number }, invalidates: [role] };
	return withForgeMutationQueue(mutationQueueKey(repository, kind, params.number, params.target), async () => {
		let original: ForgeEditable;
		try {
			original = await adapter.readEditable(repository, kind, params.number, params.target, signal);
			if (params.target.kind === "artifact") assertEditableArtifact(original, kind, params.number);
			else assertEditableComment(original, kind, params.number, params.target.id);
		} catch (error) {
			throw new ForgePreflightError(error instanceof Error ? error : new Error(String(error)));
		}
		const operation = kind === "issue" ? "Issue edit" : "Pull request edit";
		const toolName = kind === "issue" ? "edit_issue" : "edit_pr";
		invalidateCache(cache, repository, kind, params.number, [role]);

		if (params.target.kind === "artifact") {
			assertEditableArtifact(original, kind, params.number);
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
				gitlabDraftTitle(title) !== original.draft
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
				throw mutationAttemptFailure(operation, error, context);
			}
			try {
				const updated = await adapter.readEditable(repository, kind, params.number, { kind: "artifact" }, signal);
				assertEditableArtifact(updated, kind, params.number, original.url);
				if (updated.title !== title || updated.body !== body) {
					throw new Error("Updated artifact mutable content did not match the requested postimage");
				}
				if (
					repository.forge === "gitlab" &&
					original.kind === "pr" &&
					(updated.kind !== "pr" || updated.draft !== original.draft)
				) {
					throw new Error("Updated GitLab pull request changed draft state");
				}
				return mutationResult(toolName, repository, context.artifact, identity, ["artifact"], {
					target: params.target,
					fields: [...new Set(params.edits.map((edit) => edit.field))],
					replacements: params.edits.length,
				});
			} catch (error) {
				throw ambiguousMutation(operation, error, "verify", context);
			}
		}

		assertEditableComment(original, kind, params.number, params.target.id);
		const body = applyExactEdits(
			original.body,
			params.edits.map(({ oldText, newText }) => ({ oldText, newText })),
			"comment body",
		);
		let identity: Awaited<ReturnType<ForgeAdapter["updateComment"]>>;
		try {
			identity = await adapter.updateComment(
				repository,
				{ kind, number: params.number, id: params.target.id, body },
				signal,
			);
			if (identity.id !== params.target.id || identity.url !== original.url) {
				throw new Error("Forge adapter returned the wrong updated comment identity");
			}
		} catch (error) {
			throw mutationAttemptFailure(operation, error, context);
		}
		try {
			const updated = await adapter.readEditable(repository, kind, params.number, params.target, signal);
			assertEditableComment(updated, kind, params.number, params.target.id, original.url);
			if (updated.body !== body)
				throw new Error("Updated comment mutable content did not match the requested postimage");
			return mutationResult(toolName, repository, context.artifact, identity, ["comments"], {
				target: params.target,
				fields: ["body"],
				replacements: params.edits.length,
			});
		} catch (error) {
			throw ambiguousMutation(operation, error, "verify", context);
		}
	});
}

interface ForgeRenderContext {
	lastComponent?: unknown;
}

function renderCall(name: "read_issue" | "read_pr", args: ReadParams, theme: Theme, context: ForgeRenderContext) {
	const component = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
	let content = theme.fg("toolTitle", theme.bold(`${name} `)) + theme.fg("accent", `#${args.number}`);
	const options: string[] = [];
	if (args.include?.length) options.push(`include=${args.include.join(",")}`);
	if (args.offset !== undefined) options.push(`offset=${args.offset}`);
	if (args.byte_offset !== undefined) options.push(`byte_offset=${args.byte_offset}`);
	if (args.limit !== undefined) options.push(`limit=${args.limit}`);
	if (options.length > 0) content += theme.fg("dim", ` (${options.join(" ")})`);
	component.setText(content);
	return component;
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
	const component = (context.lastComponent as Text | undefined) ?? new Text("", 0, 0);
	if (options.isPartial) {
		component.setText(theme.fg("warning", "Reading forge artifact..."));
		return component;
	}
	const output = resultText(result);
	if (!isForgeReadDetails(result.details)) {
		component.setText(rawForgeReply(output));
		return component;
	}
	const summary = result.details.segments
		.map((segment) => {
			const coverage = segment.coverage;
			if (coverage?.unit === "items")
				return `${segment.id} ${coverage.offset}-${coverage.offset + Math.max(coverage.count - 1, 0)}/${coverage.totalItems}`;
			if (coverage?.unit === "bytes")
				return `${segment.id} item ${coverage.item} bytes ${coverage.offset}-${coverage.offset + coverage.bytes - 1}`;
			return `${segment.id} error`;
		})
		.join(", ");
	let content = theme.fg("success", summary);
	if (options.expanded) {
		const pretty = prettyForgeReply(output, result.details);
		content += `\n${theme.fg("toolOutput", pretty ?? rawForgeReply(output))}`;
	}
	component.setText(content);
	return component;
}

function prerequisiteWarning(repository: ForgeRepository, result: ExecResult): string | null {
	const cli = repository.forge === "github" ? "gh" : "glab";
	const name = repository.forge === "github" ? "GitHub CLI" : "GitLab CLI";
	if (result.spawnError?.code === "ENOENT")
		return `Scramjet forge tools require ${name} (${cli}), but it is not installed.`;
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
	return prerequisiteWarning(repository, await exec(cli, args, { cwd, timeout: FORGE_EXEC_TIMEOUT_MS }));
}

export function registerForgeTools(pi: ExtensionAPI, overrides: ForgeToolDependencies = {}): void {
	const dependencies: Required<ForgeToolDependencies> = {
		resolveRepository: overrides.resolveRepository ?? resolveCurrentRepository,
		createAdapter: overrides.createAdapter ?? defaultAdapter,
		logger: overrides.logger ?? { warn() {} },
	};
	const exec = forgeExec(pi);
	const cache = new Map<string, PreparedForgeRead>();

	for (const [name, kind] of [
		["read_issue", "issue"],
		["read_pr", "pr"],
	] as const) {
		pi.registerTool({
			name,
			label: kind === "issue" ? "read issue" : "read pull request",
			description: readDescription(kind),
			promptSnippet: `Read a current-repository ${kind === "issue" ? "issue" : "pull request"} conversation`,
			promptGuidelines: [
				`Continue truncated ${name} segments with the returned include, offset, byte_offset when present, and unchanged snapshot until complete.`,
			],
			parameters: kind === "issue" ? READ_ISSUE_SCHEMA : READ_PR_SCHEMA,
			async execute(_toolCallId, params, signal, _onUpdate, ctx) {
				return executeForgeTool(name, "read", () =>
					readArtifact(kind, params, signal, ctx.cwd, exec, dependencies, cache),
				);
			},
			renderCall(args, theme, context) {
				return renderCall(name, args, theme, context);
			},
			renderResult,
		});
	}

	pi.registerTool({
		name: "create_issue",
		label: "create issue",
		description: "Create and verify an issue in the current repository with an exact title and Markdown body.",
		promptSnippet: "Create a current-repository issue",
		promptGuidelines: ["Call create_issue only after exact title and body approval."],
		parameters: CREATE_ISSUE_SCHEMA,
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			return executeForgeTool("create_issue", "mutation", () =>
				createArtifact("issue", params, signal, ctx.cwd, exec, dependencies, cache),
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
				createArtifact("pr", params, signal, ctx.cwd, exec, dependencies, cache),
			);
		},
	});

	for (const [name, kind] of [
		["add_issue_comment", "issue"],
		["add_pr_comment", "pr"],
	] as const) {
		pi.registerTool({
			name,
			label: kind === "issue" ? "add issue comment" : "add pull request comment",
			description: `Add and verify one top-level ${kind === "issue" ? "issue" : "pull request"} comment after complete prior artifact and comments segments.`,
			promptSnippet: `Add a comment to a previously read ${kind === "issue" ? "issue" : "PR"}`,
			promptGuidelines: [`Before ${name}, completely read the parent artifact and comments segments.`],
			parameters: kind === "issue" ? ADD_ISSUE_COMMENT_SCHEMA : ADD_PR_COMMENT_SCHEMA,
			async execute(toolCallId, params, signal, _onUpdate, ctx) {
				return executeForgeTool(name, "mutation", () =>
					addComment(
						kind,
						params,
						toolCallId,
						signal,
						ctx.cwd,
						ctx.sessionManager.getBranch(),
						exec,
						dependencies,
						cache,
					),
				);
			},
		});
	}

	for (const [name, kind] of [
		["edit_issue", "issue"],
		["edit_pr", "pr"],
	] as const) {
		pi.registerTool({
			name,
			label: kind === "issue" ? "edit issue" : "edit pull request",
			description: `Edit one previously read ${kind === "issue" ? "issue" : "pull request"} or top-level comment using exact replacements against JSON-decoded provider field values.`,
			promptSnippet: `Exactly edit a previously read ${kind === "issue" ? "issue" : "PR"} or comment`,
			promptGuidelines: [
				`Before ${name}, completely read the target segment; replacements match decoded text exactly and target one object.`,
			],
			parameters: kind === "issue" ? EDIT_ISSUE_SCHEMA : EDIT_PR_SCHEMA,
			async execute(toolCallId, params, signal, _onUpdate, ctx) {
				return executeForgeTool(name, "mutation", () =>
					editArtifact(
						kind,
						params,
						toolCallId,
						signal,
						ctx.cwd,
						ctx.sessionManager.getBranch(),
						exec,
						dependencies,
						cache,
					),
				);
			},
		});
	}

	pi.on("session_tree", () => cache.clear());
	pi.on("session_start", (_event, ctx) => {
		cache.clear();
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
