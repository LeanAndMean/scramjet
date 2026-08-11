import type { ExecOptions, ExecResult } from "@leanandmean/coding-agent";

export type ForgeRepository =
	| { provider: "github"; owner: string; repository: string }
	| { provider: "gitlab"; namespace: string; repository: string };

export type ForgeExec = (command: string, args: string[], options?: ExecOptions) => Promise<ExecResult>;

export interface IssueProposal {
	readonly title: string;
	readonly body: string;
}

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
	) {
		throw new Error("origin is not canonical");
	}
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

export async function publishGithubIssue(
	exec: ForgeExec,
	repository: Extract<ForgeRepository, { provider: "github" }>,
	proposal: IssueProposal,
	cwd: string,
	signal?: AbortSignal,
): Promise<PublicationOutcome> {
	const endpoint = `repos/${repository.owner}/${repository.repository}/issues`;
	const mutation = await exec("gh", ["api", "--method", "POST", "--input", "-", endpoint], {
		cwd,
		signal,
		stdin: JSON.stringify({ title: proposal.title, body: proposal.body }),
	});
	if (mutation.spawnError) return { status: "no-write", reason: "mutation-process-did-not-spawn" };
	if (mutation.code !== 0 || mutation.killed || mutation.stdinError)
		return ambiguous("mutation-process-failed-after-spawn");

	const created = parseJson(mutation.stdout);
	if (!isIssueIdentity(created, repository)) return ambiguous("mutation-response-identity-invalid");
	const fetched = await exec("gh", ["api", `${endpoint}/${created.number}`], { cwd, signal });
	if (fetched.spawnError || fetched.code !== 0 || fetched.killed) return ambiguous("verification-request-failed");
	const verified = parseJson(fetched.stdout);
	if (!isFetchedIssue(verified, created.number, created.html_url, proposal)) return ambiguous("verification-mismatch");
	return { status: "verified", url: created.html_url };
}

function parseJson(text: string): unknown {
	try {
		return JSON.parse(text);
	} catch {
		return undefined;
	}
}

function isIssueIdentity(
	value: unknown,
	repository: Extract<ForgeRepository, { provider: "github" }>,
): value is { number: number; html_url: string } {
	if (!value || typeof value !== "object") return false;
	const item = value as Record<string, unknown>;
	if (!Number.isInteger(item.number) || typeof item.html_url !== "string" || "pull_request" in item) return false;
	let url: URL;
	try {
		url = new URL(item.html_url);
	} catch {
		return false;
	}
	const segments = url.pathname.split("/").filter(Boolean);
	return (
		url.protocol === "https:" &&
		url.hostname === "github.com" &&
		!url.username &&
		!url.password &&
		!url.port &&
		!url.search &&
		!url.hash &&
		segments.length === 4 &&
		segments[0]?.toLowerCase() === repository.owner.toLowerCase() &&
		segments[1]?.toLowerCase() === repository.repository.toLowerCase() &&
		segments[2] === "issues" &&
		segments[3] === String(item.number)
	);
}

function isFetchedIssue(value: unknown, number: number, url: string, proposal: IssueProposal): boolean {
	if (!value || typeof value !== "object") return false;
	const item = value as Record<string, unknown>;
	return (
		item.number === number &&
		item.html_url === url &&
		!("pull_request" in item) &&
		item.title === proposal.title &&
		item.body === proposal.body
	);
}

function ambiguous(reason: string): PublicationOutcome {
	return { status: "ambiguous", reason, retryProhibited: true };
}
