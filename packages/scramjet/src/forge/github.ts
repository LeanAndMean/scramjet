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

interface GithubAdapterContext {
	exec: ForgeExec;
	cwd: string;
}

function malformed(label: string): never {
	throw new Error(`GitHub ${label} response was malformed or incomplete`);
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

function repositoryParts(repository: ForgeRepository): void {
	if (repository.forge !== "github" || repository.host !== "github.com") {
		throw new Error("GitHub adapter requires a GitHub repository");
	}
	const parts = repository.projectPath.split("/");
	if (parts.length !== 2 || parts.some((part) => !/^[A-Za-z0-9._-]+$/.test(part))) {
		throw new Error("GitHub adapter requires a valid GitHub repository");
	}
}

function repositoriesEqual(left: string, right: string): boolean {
	return left.toLowerCase() === right.toLowerCase();
}

function artifactRepository(url: string, kind: ForgeArtifactKind, number: number, label: string): string {
	const segment = kind === "issue" ? "issues" : "pull";
	const match = new RegExp(`^https://github\\.com/([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)/${segment}/${number}$`).exec(url);
	if (match === null) return malformed(label);
	return match[1];
}

function planSegment(
	id: ForgeReadSegmentId,
	args: string[],
	shape: ForgeReadPlanSegment["shape"],
	extra: Pick<ForgeReadPlanSegment, "optional" | "evidence"> = {},
): ForgeReadPlanSegment {
	return { id, command: "gh", args, shape, ...extra };
}

function readPlan(
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	include: readonly ForgeReadSegmentId[],
): ForgeReadPlan {
	repositoryParts(repository);
	positiveInteger(number, "artifact number");
	if (new Set(include).size !== include.length || include.length === 0) {
		throw new Error("GitHub read plan requires unique segments");
	}
	const segments = include.map((id): ForgeReadPlanSegment => {
		switch (id) {
			case "artifact":
				return planSegment(
					id,
					["api", `repos/${repository.projectPath}/${kind === "issue" ? "issues" : "pulls"}/${number}`],
					{ kind: "json" },
					{ evidence: "artifact" },
				);
			case "comments":
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--slurp",
						`repos/${repository.projectPath}/issues/${number}/comments?per_page=100`,
					],
					{ kind: "gh-slurp" },
					{ evidence: "comments" },
				);
			case "sub_issues":
				if (kind !== "issue") throw new Error("GitHub pull requests do not have sub-issue segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--slurp",
						`repos/${repository.projectPath}/issues/${number}/sub_issues?per_page=100`,
					],
					{ kind: "gh-slurp" },
				);
			case "parent":
				if (kind !== "issue") throw new Error("GitHub pull requests do not have parent-issue segments");
				return planSegment(
					id,
					["api", `repos/${repository.projectPath}/issues/${number}/parent`],
					{ kind: "json" },
					{ optional: true },
				);
			case "files":
				if (kind !== "pr") throw new Error("GitHub issue reads do not have file segments");
				return planSegment(
					id,
					["api", "--paginate", "--slurp", `repos/${repository.projectPath}/pulls/${number}/files?per_page=100`],
					{ kind: "gh-slurp" },
				);
			case "commits":
				if (kind !== "pr") throw new Error("GitHub issue reads do not have commit segments");
				return planSegment(
					id,
					["api", "--paginate", "--slurp", `repos/${repository.projectPath}/pulls/${number}/commits?per_page=100`],
					{ kind: "gh-slurp" },
				);
			case "check_runs":
				if (kind !== "pr") throw new Error("GitHub issue reads do not have check-run segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--slurp",
						`repos/${repository.projectPath}/commits/refs%2Fpull%2F${number}%2Fhead/check-runs?per_page=100`,
					],
					{ kind: "gh-slurp", itemsPath: ["check_runs"] },
				);
			case "status":
				if (kind !== "pr") throw new Error("GitHub issue reads do not have status segments");
				return planSegment(
					id,
					[
						"api",
						"--paginate",
						"--slurp",
						`repos/${repository.projectPath}/commits/refs%2Fpull%2F${number}%2Fhead/status?per_page=100`,
					],
					{ kind: "gh-slurp", itemsPath: ["statuses"] },
				);
			default:
				throw new Error(`GitHub does not support the ${id} forge segment`);
		}
	});
	return { repository, artifact: { kind, number }, include: [...include], segments };
}

async function githubCommand(
	context: GithubAdapterContext,
	args: string[],
	signal?: AbortSignal,
	stdin?: string,
): Promise<string> {
	const result = await runForgeCommand(context.exec, {
		command: "gh",
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
	const artifact = record(value, kind === "issue" ? "issue" : "pull request");
	if (positiveInteger(artifact.number, "artifact") !== number) return malformed("artifact");
	if (kind === "issue" && artifact.pull_request !== undefined) return malformed("issue");
	const url = string(artifact.html_url, "artifact");
	if (!repositoriesEqual(artifactRepository(url, kind, number, "artifact"), repository.projectPath)) {
		return malformed("artifact");
	}
	const common = {
		target: { kind: "artifact" as const },
		kind,
		number,
		url,
		title: string(artifact.title, "artifact"),
		body: nullableString(artifact.body, "artifact"),
	};
	if (kind === "issue") return { ...common, kind };
	return {
		...common,
		kind,
		draft: boolean(artifact.draft, "pull request"),
		head: string(record(artifact.head, "pull request").ref, "pull request"),
		base: string(record(artifact.base, "pull request").ref, "pull request"),
	};
}

function parseComment(
	value: unknown,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	expectedId: string,
): Extract<ForgeEditable, { target: { kind: "comment" } }> {
	const comment = record(value, "comment");
	const id = decimalId(comment.id, "comment");
	if (id !== expectedId) return malformed("comment");
	const issueUrl = string(comment.issue_url, "comment");
	const issueMatch =
		/^https:\/\/api\.github\.com\/repos\/([A-Za-z0-9._-]+\/[A-Za-z0-9._-]+)\/issues\/([1-9]\d*)$/.exec(issueUrl);
	const url = string(comment.html_url, "comment");
	const segment = kind === "issue" ? "issues" : "pull";
	const commentMatch = new RegExp(
		`^https://github\\.com/([A-Za-z0-9._-]+/[A-Za-z0-9._-]+)/${segment}/${number}#issuecomment-${id}$`,
	).exec(url);
	if (
		issueMatch === null ||
		commentMatch === null ||
		Number(issueMatch[2]) !== number ||
		!repositoriesEqual(issueMatch[1], repository.projectPath) ||
		!repositoriesEqual(commentMatch[1], repository.projectPath)
	) {
		return malformed("comment");
	}
	return {
		target: { kind: "comment", id },
		kind,
		number,
		url,
		body: string(comment.body, "comment"),
	};
}

async function readEditable(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	target: ForgeMutationTarget,
	signal?: AbortSignal,
): Promise<ForgeEditable> {
	repositoryParts(repository);
	positiveInteger(number, "artifact number");
	if (target.kind === "artifact") {
		const endpoint = `repos/${repository.projectPath}/${kind === "issue" ? "issues" : "pulls"}/${number}`;
		return parseArtifact(
			parseJson(await githubCommand(context, ["api", endpoint], signal), "artifact"),
			repository,
			kind,
			number,
		);
	}
	if (!/^[1-9]\d*$/.test(target.id)) throw new Error("GitHub comment ID must be a positive decimal string");
	return parseComment(
		parseJson(
			await githubCommand(context, ["api", `repos/${repository.projectPath}/issues/comments/${target.id}`], signal),
			"comment",
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
	const number = positiveInteger(response.number, "mutation response");
	const url = string(response.html_url, "mutation response");
	if (
		(expectedNumber !== undefined && number !== expectedNumber) ||
		!repositoriesEqual(artifactRepository(url, kind, number, "mutation response"), repository.projectPath)
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
	const editable = parseComment(
		value,
		repository,
		kind,
		parentNumber,
		expectedId ?? decimalId(record(value, "mutation response").id, "mutation response"),
	);
	return { kind: "comment", id: editable.target.id, url: editable.url };
}

async function mutate(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	method: "POST" | "PATCH",
	endpoint: string,
	payload: Record<string, unknown>,
	signal?: AbortSignal,
): Promise<unknown> {
	repositoryParts(repository);
	return parseJson(
		await githubCommand(
			context,
			["api", "--method", method, "--input", "-", endpoint],
			signal,
			JSON.stringify(payload),
		),
		"mutation response",
	);
}

export function createGithubAdapter(exec: ForgeExec, cwd: string): ForgeAdapter {
	const context = { exec, cwd };
	return {
		readPlan,
		readEditable: (repository, kind, number, target, signal) =>
			readEditable(context, repository, kind, number, target, signal),
		async createArtifact(repository, input: ForgeCreateInput, signal) {
			const endpoint = `repos/${repository.projectPath}/${input.kind === "issue" ? "issues" : "pulls"}`;
			const payload =
				input.kind === "issue"
					? { title: input.title, body: input.body }
					: { title: input.title, body: input.body, head: input.head, base: input.base, draft: input.draft };
			return mutationIdentity(
				await mutate(context, repository, "POST", endpoint, payload, signal),
				repository,
				input.kind,
			);
		},
		async updateArtifact(repository, input, signal) {
			positiveInteger(input.number, "artifact number");
			if (input.title === undefined && input.body === undefined) throw new Error("Artifact update requires content");
			const endpoint = `repos/${repository.projectPath}/${input.kind === "issue" ? "issues" : "pulls"}/${input.number}`;
			const payload = {
				...(input.title === undefined ? {} : { title: input.title }),
				...(input.body === undefined ? {} : { body: input.body }),
			};
			return mutationIdentity(
				await mutate(context, repository, "PATCH", endpoint, payload, signal),
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
					`repos/${repository.projectPath}/issues/${input.number}/comments`,
					{ body: input.body },
					signal,
				),
				repository,
				input.kind,
				input.number,
			);
		},
		async updateComment(repository, input, signal) {
			if (!/^[1-9]\d*$/.test(input.id)) throw new Error("GitHub comment ID must be a positive decimal string");
			return commentIdentity(
				await mutate(
					context,
					repository,
					"PATCH",
					`repos/${repository.projectPath}/issues/comments/${input.id}`,
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
