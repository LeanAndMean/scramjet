import { FORGE_EXEC_TIMEOUT_MS, type ForgeExec, runForgeCommand } from "./client.js";
import type {
	ForgeAdapter,
	ForgeArtifactIdentity,
	ForgeArtifactKind,
	ForgeCommentIdentity,
	ForgeCreateInput,
	ForgeEditable,
	ForgeMutationTarget,
	ForgeReadPlan,
	ForgeReadPlanSegment,
	ForgeReadSegmentId,
	ForgeRepository,
} from "./types.js";

const HIERARCHY_QUERY = `query IssueHierarchy($fullPath: ID!, $iid: String!, $endCursor: String) {
  project(fullPath: $fullPath) {
    workItems(iid: $iid, first: 1) {
      count
      nodes {
        iid
        widgets(onlyTypes: [HIERARCHY]) {
          ... on WorkItemWidgetHierarchy {
            parent { iid webUrl state title }
            children(first: 100, after: $endCursor) {
              count
              nodes { iid webUrl state title }
              pageInfo { hasNextPage endCursor }
            }
          }
        }
      }
    }
  }
}`;

interface GitlabAdapterContext {
	exec: ForgeExec;
	cwd: string;
}

function malformed(label: string): never {
	throw new Error(`GitLab ${label} response was malformed or incomplete`);
}

function record(value: unknown, label: string): Record<string, unknown> {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return malformed(label);
	return value as Record<string, unknown>;
}

function string(value: unknown, label: string): string {
	if (typeof value !== "string") return malformed(label);
	return value;
}

function nullableString(value: unknown, label: string): string {
	return value === null ? "" : string(value, label);
}

function boolean(value: unknown, label: string): boolean {
	if (typeof value !== "boolean") return malformed(label);
	return value;
}

function positiveInteger(value: unknown, label: string): number {
	if (!Number.isSafeInteger(value) || (value as number) <= 0) return malformed(label);
	return value as number;
}

function decimalId(value: unknown, label: string): string {
	if (typeof value === "string" && /^[1-9]\d*$/.test(value)) return value;
	if (Number.isSafeInteger(value) && (value as number) > 0) return String(value);
	return malformed(label);
}

function parseJson(stdout: string, label: string): unknown {
	try {
		return JSON.parse(stdout);
	} catch {
		return malformed(label);
	}
}

function projectId(repository: ForgeRepository): string {
	if (repository.forge !== "gitlab" || repository.host !== "gitlab.com") {
		throw new Error("GitLab adapter requires a GitLab repository");
	}
	const segments = repository.projectPath.split("/");
	if (segments.length < 2 || segments.some((segment) => !/^[A-Za-z0-9._-]+$/.test(segment))) {
		throw new Error("GitLab adapter requires a valid GitLab repository");
	}
	return encodeURIComponent(repository.projectPath);
}

function artifactPath(kind: ForgeArtifactKind): "issues" | "merge_requests" {
	return kind === "issue" ? "issues" : "merge_requests";
}

function validArtifactUrl(repository: ForgeRepository, kind: ForgeArtifactKind, number: number, url: string): boolean {
	if (kind === "pr") return url === `https://gitlab.com/${repository.projectPath}/-/merge_requests/${number}`;
	return (
		url === `https://gitlab.com/${repository.projectPath}/-/issues/${number}` ||
		url === `https://gitlab.com/${repository.projectPath}/-/work_items/${number}`
	);
}

function planSegment(
	id: ForgeReadSegmentId,
	args: string[],
	shape: ForgeReadPlanSegment["shape"],
	evidence?: ForgeReadPlanSegment["evidence"],
): ForgeReadPlanSegment {
	return { id, command: "glab", args, shape, ...(evidence === undefined ? {} : { evidence }) };
}

function readPlan(
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	include: readonly ForgeReadSegmentId[],
): ForgeReadPlan {
	const project = projectId(repository);
	positiveInteger(number, "artifact number");
	if (new Set(include).size !== include.length || include.length === 0) {
		throw new Error("GitLab read plan requires unique segments");
	}
	const segments = include.map((id): ForgeReadPlanSegment => {
		switch (id) {
			case "artifact":
				return planSegment(
					id,
					["api", `projects/${project}/${artifactPath(kind)}/${number}`],
					{ kind: "json" },
					"artifact",
				);
			case "comments":
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--output",
						"ndjson",
						`projects/${project}/${artifactPath(kind)}/${number}/notes?per_page=100&sort=asc&order_by=created_at`,
					],
					{ kind: "ndjson" },
					"comments",
				);
			case "relationships":
				if (kind !== "issue") throw new Error("GitLab merge requests do not have relationship segments");
				return planSegment(
					id,
					[
						"api",
						"graphql",
						"--paginate",
						"--output",
						"ndjson",
						"-f",
						`query=${HIERARCHY_QUERY}`,
						"-f",
						`fullPath=${repository.projectPath}`,
						"-f",
						`iid=${number}`,
					],
					{ kind: "ndjson" },
				);
			case "files":
				if (kind !== "pr") throw new Error("GitLab issue reads do not have file segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--output",
						"ndjson",
						`projects/${project}/merge_requests/${number}/diffs?per_page=100`,
					],
					{ kind: "ndjson" },
				);
			case "commits":
				if (kind !== "pr") throw new Error("GitLab issue reads do not have commit segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--output",
						"ndjson",
						`projects/${project}/merge_requests/${number}/commits?per_page=100`,
					],
					{ kind: "ndjson" },
				);
			case "pipelines":
				if (kind !== "pr") throw new Error("GitLab issue reads do not have pipeline segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--output",
						"ndjson",
						`projects/${project}/merge_requests/${number}/pipelines?per_page=100`,
					],
					{ kind: "ndjson" },
				);
			default:
				throw new Error(`GitLab does not support the ${id} forge segment`);
		}
	});
	return { repository, artifact: { kind, number }, include: [...include], segments };
}

async function gitlabCommand(
	context: GitlabAdapterContext,
	args: string[],
	signal?: AbortSignal,
	stdin?: string,
): Promise<string> {
	const result = await runForgeCommand(context.exec, {
		command: "glab",
		args,
		cwd: context.cwd,
		stdin,
		signal,
		timeout: FORGE_EXEC_TIMEOUT_MS,
	});
	return result.stdout;
}

function parseArtifact(
	value: unknown,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): Extract<ForgeEditable, { target: { kind: "artifact" } }> {
	const artifact = record(value, kind === "issue" ? "issue" : "merge request");
	if (positiveInteger(artifact.iid, "artifact") !== number) return malformed("artifact");
	positiveInteger(artifact.project_id, "artifact");
	const url = string(artifact.web_url, "artifact");
	if (!validArtifactUrl(repository, kind, number, url)) return malformed("artifact");
	const common = {
		target: { kind: "artifact" as const },
		kind,
		number,
		url,
		title: string(artifact.title, "artifact"),
		body: nullableString(artifact.description, "artifact"),
	};
	if (kind === "issue") return { ...common, kind };
	return {
		...common,
		kind,
		draft: boolean(artifact.draft, "merge request"),
		head: string(artifact.source_branch, "merge request"),
		base: string(artifact.target_branch, "merge request"),
	};
}

function parseComment(
	value: unknown,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	expectedId: string,
): Extract<ForgeEditable, { target: { kind: "comment" } }> {
	const note = record(value, "note");
	const id = decimalId(note.id, "note");
	if (
		id !== expectedId ||
		positiveInteger(note.noteable_iid, "note") !== number ||
		string(note.noteable_type, "note") !== (kind === "issue" ? "Issue" : "MergeRequest") ||
		boolean(note.system, "note") ||
		note.position !== null ||
		(note.type !== null &&
			note.type !== undefined &&
			["DiscussionNote", "DiffNote"].includes(string(note.type, "note")))
	) {
		return malformed("note");
	}
	const parentUrl = string(note.noteable_url ?? note.web_url ?? "", "note");
	const url = parentUrl.includes("#note_")
		? parentUrl
		: `https://gitlab.com/${repository.projectPath}/-/${kind === "issue" ? "work_items" : "merge_requests"}/${number}#note_${id}`;
	return {
		target: { kind: "comment", id },
		kind,
		number,
		url,
		body: string(note.body, "note"),
	};
}

async function readEditable(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	target: ForgeMutationTarget,
	signal?: AbortSignal,
): Promise<ForgeEditable> {
	const project = projectId(repository);
	positiveInteger(number, "artifact number");
	if (target.kind === "artifact") {
		return parseArtifact(
			parseJson(
				await gitlabCommand(context, ["api", `projects/${project}/${artifactPath(kind)}/${number}`], signal),
				"artifact",
			),
			repository,
			kind,
			number,
		);
	}
	if (!/^[1-9]\d*$/.test(target.id)) throw new Error("GitLab note ID must be a positive decimal string");
	return parseComment(
		parseJson(
			await gitlabCommand(
				context,
				["api", `projects/${project}/${artifactPath(kind)}/${number}/notes/${target.id}`],
				signal,
			),
			"note",
		),
		repository,
		kind,
		number,
		target.id,
	);
}

function mutationIdentity(
	value: unknown,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	expectedNumber?: number,
): ForgeArtifactIdentity {
	const response = record(value, "mutation response");
	const number = positiveInteger(response.iid, "mutation response");
	positiveInteger(response.project_id, "mutation response");
	const url = string(response.web_url, "mutation response");
	if (
		(expectedNumber !== undefined && number !== expectedNumber) ||
		!validArtifactUrl(repository, kind, number, url)
	) {
		return malformed("mutation response");
	}
	return { kind, number, url };
}

function commentIdentity(
	value: unknown,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	parentNumber: number,
	expectedId?: string,
): ForgeCommentIdentity {
	const response = record(value, "mutation response");
	const id = decimalId(response.id, "mutation response");
	positiveInteger(response.project_id, "mutation response");
	positiveInteger(response.noteable_id, "mutation response");
	if (
		(expectedId !== undefined && id !== expectedId) ||
		positiveInteger(response.noteable_iid, "mutation response") !== parentNumber ||
		string(response.noteable_type, "mutation response") !== (kind === "issue" ? "Issue" : "MergeRequest")
	) {
		return malformed("mutation response");
	}
	return {
		kind: "comment",
		id,
		url: `https://gitlab.com/${repository.projectPath}/-/${
			kind === "issue" ? "work_items" : "merge_requests"
		}/${parentNumber}#note_${id}`,
	};
}

async function mutate(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	method: "POST" | "PUT",
	endpoint: string,
	payload: Record<string, unknown>,
	signal?: AbortSignal,
): Promise<unknown> {
	projectId(repository);
	return parseJson(
		await gitlabCommand(
			context,
			["api", "--method", method, "--input", "-", endpoint],
			signal,
			JSON.stringify(payload),
		),
		"mutation response",
	);
}

const DRAFT_PREFIX = /^(?:Draft:|WIP:|\[(?:Draft|WIP)\]|\((?:Draft|WIP)\))\s*/i;

function mergeRequestTitle(title: string, draft: boolean): string {
	const prefixed = DRAFT_PREFIX.test(title);
	if (!draft && prefixed)
		throw new Error("GitLab cannot create a non-draft merge request with a draft-prefixed title");
	if (draft && !prefixed) {
		throw new Error("GitLab draft merge requests require the exact approved title to include a draft prefix");
	}
	return title;
}

export function createGitlabAdapter(exec: ForgeExec, cwd: string): ForgeAdapter {
	const context = { exec, cwd };
	return {
		readPlan,
		readEditable: (repository, kind, number, target, signal) =>
			readEditable(context, repository, kind, number, target, signal),
		async createArtifact(repository, input: ForgeCreateInput, signal) {
			const endpoint = `projects/${projectId(repository)}/${artifactPath(input.kind)}`;
			const payload =
				input.kind === "issue"
					? { title: input.title, description: input.body }
					: {
							title: mergeRequestTitle(input.title, input.draft),
							description: input.body,
							source_branch: input.head,
							target_branch: input.base,
						};
			return mutationIdentity(
				await mutate(context, repository, "POST", endpoint, payload, signal),
				repository,
				input.kind,
			);
		},
		async updateArtifact(repository, input, signal) {
			if (input.title === undefined && input.body === undefined) throw new Error("Artifact update requires content");
			const endpoint = `projects/${projectId(repository)}/${artifactPath(input.kind)}/${input.number}`;
			const payload = {
				...(input.title === undefined ? {} : { title: input.title }),
				...(input.body === undefined ? {} : { description: input.body }),
			};
			return mutationIdentity(
				await mutate(context, repository, "PUT", endpoint, payload, signal),
				repository,
				input.kind,
				input.number,
			);
		},
		async addComment(repository, input, signal) {
			return commentIdentity(
				await mutate(
					context,
					repository,
					"POST",
					`projects/${projectId(repository)}/${artifactPath(input.kind)}/${input.number}/notes`,
					{ body: input.body },
					signal,
				),
				repository,
				input.kind,
				input.number,
			);
		},
		async updateComment(repository, input, signal) {
			if (!/^[1-9]\d*$/.test(input.id)) throw new Error("GitLab note ID must be a positive decimal string");
			return commentIdentity(
				await mutate(
					context,
					repository,
					"PUT",
					`projects/${projectId(repository)}/${artifactPath(input.kind)}/${input.number}/notes/${input.id}`,
					{ body: input.body },
					signal,
				),
				repository,
				input.kind,
				input.number,
				input.id,
			);
		},
	};
}
