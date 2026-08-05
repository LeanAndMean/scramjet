import { StringEnum } from "@leanandmean/ai";
import type {
	AgentToolResult,
	ExecResult,
	ExtensionAPI,
	Theme,
	ToolRenderResultOptions,
} from "@leanandmean/coding-agent";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES } from "@leanandmean/coding-agent";
import { Text } from "@leanandmean/tui";
import { type Static, Type } from "typebox";
import { FORGE_EXEC_TIMEOUT_MS, type ForgeExec, resolveCurrentRepository } from "./client.js";
import { type ForgeRangeRequest, isForgeReadDetails, renderForgeDocument, sliceForgeDocument } from "./document.js";
import { createGithubAdapter } from "./github.js";
import { createGitlabAdapter } from "./gitlab.js";
import type { ForgeAdapter, ForgeArtifactKind, ForgePrSection, ForgeRepository } from "./types.js";

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

type ReadIssueParams = Static<typeof READ_ISSUE_SCHEMA>;
type ReadPrParams = Static<typeof READ_PR_SCHEMA>;

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

	pi.on("session_start", (_event, ctx) => {
		if (!ctx.hasUI) return;
		void probePrerequisite(exec, ctx.cwd, dependencies.resolveRepository)
			.then((warning) => {
				if (warning !== null) ctx.ui.notify(warning, "warning");
			})
			.catch(() => {});
	});
}
