import type { ExecOptions, ExecResult } from "@leanandmean/coding-agent";

export type ForgeRepository =
	| { provider: "github"; owner: string; repository: string }
	| { provider: "gitlab"; namespace: string; repository: string };
export type ForgeExec = (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;
export type PublicationRequest =
	| { readonly operation: "create_issue"; readonly title: string; readonly body: string }
	| {
			readonly operation: "create_pr";
			readonly title: string;
			readonly body: string;
			readonly head: string;
			readonly base: string;
			readonly draft: boolean;
	  }
	| { readonly operation: "add_issue_comment"; readonly number: number; readonly body: string }
	| { readonly operation: "add_pr_comment"; readonly number: number; readonly body: string };
export type PublicationOutcome =
	| { status: "verified"; url: string }
	| { status: "no-write"; reason: string }
	| { status: "ambiguous"; reason: string; retryProhibited: true };

export function parseForgeOrigin(input: string): ForgeRepository {
	if (
		input !== input.trim() ||
		input.includes("%") ||
		/\/(?:\.{1,2})(?:\/|$)/.test(input) ||
		/^(?:https|ssh):\/\/[^/]*:\d+(?:\/|$)/.test(input)
	)
		throw new Error("origin is not canonical");
	const scp = /^git@(github\.com|gitlab\.com):([^?#]+)$/.exec(input);
	if (scp) return repositoryFromPath(scp[1]!, scp[2]!);
	let url: URL;
	try {
		url = new URL(input);
	} catch {
		throw new Error("origin is not a supported URL");
	}
	if (url.protocol !== "https:" && url.protocol !== "ssh:") throw new Error("origin protocol is unsupported");
	if (url.hostname !== "github.com" && url.hostname !== "gitlab.com") throw new Error("origin host is unsupported");
	if (url.port || url.search || url.hash || url.password)
		throw new Error("origin contains unsupported URL components");
	if (url.protocol === "https:" && url.username) throw new Error("HTTPS origin must not contain credentials");
	if (url.protocol === "ssh:" && url.username !== "git") throw new Error("SSH origin must use the git user");
	return repositoryFromPath(url.hostname, url.pathname);
}
function repositoryFromPath(host: string, rawPath: string): ForgeRepository {
	let path = rawPath.replace(/^\//, "");
	if (path.endsWith(".git")) path = path.slice(0, -4);
	const segments = path.split("/");
	if (segments.some((segment) => !segment || segment === "." || segment === ".."))
		throw new Error("origin path is malformed");
	if (host === "github.com") {
		if (segments.length !== 2) throw new Error("GitHub origin must contain owner and repository");
		return { provider: "github", owner: segments[0]!, repository: segments[1]! };
	}
	if (segments.length < 2) throw new Error("GitLab origin must contain namespace and repository");
	return { provider: "gitlab", namespace: segments.slice(0, -1).join("/"), repository: segments.at(-1)! };
}
export function sameRepository(left: ForgeRepository, right: ForgeRepository): boolean {
	return JSON.stringify(left) === JSON.stringify(right);
}
export async function resolveForgeOrigin(exec: ForgeExec, cwd: string): Promise<ForgeRepository> {
	const result = await exec("git", ["remote", "get-url", "origin"], { cwd });
	if (result.spawnError || result.code !== 0) throw new Error("Unable to read the current repository origin");
	return parseForgeOrigin(result.stdout.trimEnd());
}

export async function preflightPullRequestBranches(
	exec: ForgeExec,
	request: Extract<PublicationRequest, { operation: "create_pr" }>,
	cwd: string,
): Promise<void> {
	for (const branch of [request.head, request.base]) {
		if (!branch || branch.includes(":")) throw new Error("PR branches must be branches in the current repository");
		const checked = await exec("git", ["show-ref", "--verify", "--quiet", `refs/remotes/origin/${branch}`], { cwd });
		if (checked.spawnError || checked.code !== 0) throw new Error(`Remote branch does not exist: ${branch}`);
	}
}

export async function publishForge(
	exec: ForgeExec,
	repository: ForgeRepository,
	request: PublicationRequest,
	cwd: string,
	signal?: AbortSignal,
): Promise<PublicationOutcome> {
	return repository.provider === "github"
		? publishGithub(exec, repository, request, cwd, signal)
		: publishGitlab(exec, repository, request, cwd, signal);
}

async function invoke(
	exec: ForgeExec,
	cli: "gh" | "glab",
	endpoint: string,
	payload: object,
	cwd: string,
	signal?: AbortSignal,
) {
	const result = await exec(cli, ["api", "--method", "POST", "--input", "-", endpoint], {
		cwd,
		signal,
		stdin: JSON.stringify(payload),
	});
	if (result.spawnError)
		return { outcome: { status: "no-write", reason: "mutation-process-did-not-spawn" } as PublicationOutcome };
	if (result.code !== 0 || result.killed || result.stdinError)
		return { outcome: ambiguous("mutation-process-failed-after-spawn") };
	return { value: parseJson(result.stdout) };
}
async function fetchJson(exec: ForgeExec, cli: "gh" | "glab", endpoint: string, cwd: string, signal?: AbortSignal) {
	const result = await exec(cli, ["api", endpoint], { cwd, signal });
	return result.spawnError || result.code !== 0 || result.killed ? undefined : parseJson(result.stdout);
}

async function publishGithub(
	exec: ForgeExec,
	repo: Extract<ForgeRepository, { provider: "github" }>,
	request: PublicationRequest,
	cwd: string,
	signal?: AbortSignal,
): Promise<PublicationOutcome> {
	const root = `repos/${repo.owner}/${repo.repository}`;
	let endpoint: string;
	let payload: object;
	if (request.operation === "create_issue") {
		endpoint = `${root}/issues`;
		payload = { title: request.title, body: request.body };
	} else if (request.operation === "create_pr") {
		endpoint = `${root}/pulls`;
		payload = {
			title: request.title,
			body: request.body,
			head: request.head,
			base: request.base,
			draft: request.draft,
		};
	} else {
		endpoint = `${root}/issues/${request.number}/comments`;
		payload = { body: request.body };
	}
	const mutation = await invoke(exec, "gh", endpoint, payload, cwd, signal);
	if (mutation.outcome) return mutation.outcome;
	const item = record(mutation.value);
	if (!item) return ambiguous("mutation-response-identity-invalid");
	if (request.operation === "create_issue" || request.operation === "create_pr") {
		if (
			!Number.isInteger(item.number) ||
			typeof item.html_url !== "string" ||
			!githubArtifactUrl(item.html_url, repo, request.operation, item.number as number)
		)
			return ambiguous("mutation-response-identity-invalid");
		const fetched = await fetchJson(
			exec,
			"gh",
			`${root}/${request.operation === "create_pr" ? "pulls" : "issues"}/${item.number}`,
			cwd,
			signal,
		);
		if (!fetched) return ambiguous("verification-request-failed");
		const value = record(fetched);
		if (
			!value ||
			value.number !== item.number ||
			value.html_url !== item.html_url ||
			value.title !== request.title ||
			value.body !== request.body
		)
			return ambiguous("verification-mismatch");
		if (request.operation === "create_issue" && "pull_request" in value) return ambiguous("verification-mismatch");
		if (request.operation === "create_pr") {
			const head = record(value.head);
			const base = record(value.base);
			const expectedRepository = `${repo.owner}/${repo.repository}`.toLowerCase();
			if (
				!head ||
				!base ||
				head.ref !== request.head ||
				base.ref !== request.base ||
				typeof record(head.repo)?.full_name !== "string" ||
				(record(head.repo)!.full_name as string).toLowerCase() !== expectedRepository ||
				typeof record(base.repo)?.full_name !== "string" ||
				(record(base.repo)!.full_name as string).toLowerCase() !== expectedRepository ||
				value.draft !== request.draft
			)
				return ambiguous("verification-mismatch");
		}
		return { status: "verified", url: item.html_url };
	}
	if (
		!Number.isInteger(item.id) ||
		typeof item.html_url !== "string" ||
		!githubCommentUrl(item.html_url, repo, request, item.id as number)
	)
		return ambiguous("mutation-response-identity-invalid");
	const fetched = record(await fetchJson(exec, "gh", `${root}/issues/comments/${item.id}`, cwd, signal));
	if (!fetched) return ambiguous("verification-request-failed");
	if (
		fetched.id !== item.id ||
		fetched.html_url !== item.html_url ||
		fetched.body !== request.body ||
		!githubIssueApiUrl(fetched.issue_url, repo, request.number)
	)
		return ambiguous("verification-mismatch");
	return { status: "verified", url: item.html_url };
}

async function publishGitlab(
	exec: ForgeExec,
	repo: Extract<ForgeRepository, { provider: "gitlab" }>,
	request: PublicationRequest,
	cwd: string,
	signal?: AbortSignal,
): Promise<PublicationOutcome> {
	const project = `${repo.namespace}/${repo.repository}`;
	const root = `projects/${encodeURIComponent(project)}`;
	let endpoint: string;
	let payload: object;
	if (request.operation === "create_issue") {
		endpoint = `${root}/issues`;
		payload = { title: request.title, description: request.body };
	} else if (request.operation === "create_pr") {
		endpoint = `${root}/merge_requests`;
		payload = {
			title: request.title,
			description: request.body,
			source_branch: request.head,
			target_branch: request.base,
		};
	} else {
		endpoint = `${root}/${request.operation === "add_pr_comment" ? "merge_requests" : "issues"}/${request.number}/notes`;
		payload = { body: request.body };
	}
	const mutation = await invoke(exec, "glab", endpoint, payload, cwd, signal);
	if (mutation.outcome) return mutation.outcome;
	const item = record(mutation.value);
	if (!item) return ambiguous("mutation-response-identity-invalid");
	if (request.operation === "create_issue" || request.operation === "create_pr") {
		if (
			!Number.isInteger(item.iid) ||
			typeof item.web_url !== "string" ||
			!gitlabArtifactUrl(item.web_url, repo, request.operation, item.iid as number)
		)
			return ambiguous("mutation-response-identity-invalid");
		const fetched = record(
			await fetchJson(
				exec,
				"glab",
				`${root}/${request.operation === "create_pr" ? "merge_requests" : "issues"}/${item.iid}`,
				cwd,
				signal,
			),
		);
		if (!fetched) return ambiguous("verification-request-failed");
		if (
			fetched.iid !== item.iid ||
			fetched.web_url !== item.web_url ||
			fetched.title !== request.title ||
			fetched.description !== request.body
		)
			return ambiguous("verification-mismatch");
		if (
			request.operation === "create_pr" &&
			(fetched.source_branch !== request.head ||
				fetched.target_branch !== request.base ||
				!Number.isInteger(fetched.project_id) ||
				fetched.source_project_id !== fetched.project_id ||
				fetched.target_project_id !== fetched.project_id ||
				fetched.draft !== request.draft)
		)
			return ambiguous("verification-mismatch");
		return { status: "verified", url: item.web_url };
	}
	if (
		!Number.isInteger(item.id) ||
		!Number.isInteger(item.project_id) ||
		item.noteable_iid !== request.number ||
		item.noteable_type !== (request.operation === "add_pr_comment" ? "MergeRequest" : "Issue")
	)
		return ambiguous("mutation-response-identity-invalid");
	const fetched = record(await fetchJson(exec, "glab", `${endpoint}/${item.id}`, cwd, signal));
	if (!fetched) return ambiguous("verification-request-failed");
	if (
		fetched.id !== item.id ||
		fetched.project_id !== item.project_id ||
		fetched.body !== request.body ||
		fetched.noteable_iid !== request.number ||
		fetched.noteable_type !== (request.operation === "add_pr_comment" ? "MergeRequest" : "Issue")
	)
		return ambiguous("verification-mismatch");
	return { status: "verified", url: `${gitlabParentUrl(repo, request)}#note_${item.id}` };
}

function githubArtifactUrl(
	input: string,
	repo: Extract<ForgeRepository, { provider: "github" }>,
	operation: "create_issue" | "create_pr",
	number: number,
): boolean {
	return exactUrl(
		input,
		"github.com",
		[repo.owner, repo.repository, operation === "create_pr" ? "pull" : "issues", String(number)],
		true,
	);
}
function githubCommentUrl(
	input: string,
	repo: Extract<ForgeRepository, { provider: "github" }>,
	request: Extract<PublicationRequest, { operation: "add_issue_comment" | "add_pr_comment" }>,
	id: number,
): boolean {
	try {
		const url = new URL(input);
		return (
			exactUrlBase(
				url,
				"github.com",
				[
					repo.owner,
					repo.repository,
					request.operation === "add_pr_comment" ? "pull" : "issues",
					String(request.number),
				],
				true,
			) && url.hash === `#issuecomment-${id}`
		);
	} catch {
		return false;
	}
}
function githubIssueApiUrl(
	input: unknown,
	repo: Extract<ForgeRepository, { provider: "github" }>,
	number: number,
): boolean {
	return (
		typeof input === "string" &&
		exactUrl(input, "api.github.com", ["repos", repo.owner, repo.repository, "issues", String(number)], true)
	);
}
function gitlabArtifactUrl(
	input: string,
	repo: Extract<ForgeRepository, { provider: "gitlab" }>,
	operation: "create_issue" | "create_pr",
	iid: number,
): boolean {
	return exactUrl(
		input,
		"gitlab.com",
		[
			...repo.namespace.split("/"),
			repo.repository,
			"-",
			operation === "create_pr" ? "merge_requests" : "issues",
			String(iid),
		],
		false,
	);
}
function gitlabParentUrl(
	repo: Extract<ForgeRepository, { provider: "gitlab" }>,
	request: Extract<PublicationRequest, { operation: "add_issue_comment" | "add_pr_comment" }>,
): string {
	return `https://gitlab.com/${repo.namespace}/${repo.repository}/-/${request.operation === "add_pr_comment" ? "merge_requests" : "issues"}/${request.number}`;
}
function exactUrl(input: string, host: string, segments: string[], caseInsensitive: boolean): boolean {
	try {
		return exactUrlBase(new URL(input), host, segments, caseInsensitive) && !new URL(input).hash;
	} catch {
		return false;
	}
}
function exactUrlBase(url: URL, host: string, segments: string[], ci: boolean): boolean {
	const actual = url.pathname.split("/").filter(Boolean);
	const norm = (v: string) => (ci ? v.toLowerCase() : v);
	return (
		url.protocol === "https:" &&
		url.hostname === host &&
		!url.username &&
		!url.password &&
		!url.port &&
		!url.search &&
		actual.length === segments.length &&
		actual.every((v, i) => norm(v) === norm(segments[i]!))
	);
}
function parseJson(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch {
		return undefined;
	}
}
function record(value: unknown): Record<string, unknown> | undefined {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : undefined;
}
function ambiguous(reason: string): PublicationOutcome {
	return { status: "ambiguous", reason, retryProhibited: true };
}
