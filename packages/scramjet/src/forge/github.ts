import { FORGE_EXEC_TIMEOUT_MS, ForgeCommandError, type ForgeExec, runForgeCommand } from "./client.js";
import type {
	ForgeActor,
	ForgeAdapter,
	ForgeArtifact,
	ForgeArtifactIdentity,
	ForgeArtifactKind,
	ForgeComment,
	ForgeCommentIdentity,
	ForgeCreateInput,
	ForgeIssueRelationship,
	ForgePrCheck,
	ForgePrCommit,
	ForgePrFile,
	ForgePrSection,
	ForgeRepository,
} from "./types.js";

const ISSUE_QUERY = `query Issue($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      number url state author { login __typename } createdAt updatedAt
      labels(first: 100) { totalCount nodes { name } }
      assignees(first: 100) { totalCount nodes { login __typename } }
      title body
      comments(first: 100, after: $endCursor) {
        totalCount
        nodes { databaseId url author { login __typename } body createdAt updatedAt }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}`;

const PR_QUERY = `query PullRequest($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number url state author { login __typename } createdAt updatedAt
      labels(first: 100) { totalCount nodes { name } }
      assignees(first: 100) { totalCount nodes { login __typename } }
      title body isDraft mergeable reviewDecision headRefName baseRefName changedFiles
      comments(first: 100, after: $endCursor) {
        totalCount
        nodes { databaseId url author { login __typename } body createdAt updatedAt }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}`;

const RELATIONSHIPS_QUERY = `query IssueRelationships($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    issue(number: $number) {
      number
      parent { number url state title }
      subIssues(first: 100, after: $endCursor) {
        totalCount
        nodes { number url state title }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}`;

const TASK_LIST_ISSUE_QUERY = `query TaskListIssue($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    task: issue(number: $number) { number url state title }
  }
}`;

const COMMITS_QUERY = `query PullRequestCommits($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number
      commits(first: 100, after: $endCursor) {
        totalCount
        nodes { commit { oid messageHeadline authoredDate author { name } url } }
        pageInfo { hasNextPage endCursor }
      }
    }
  }
}`;

const CHECKS_QUERY = `query PullRequestChecks($owner: String!, $name: String!, $number: Int!, $endCursor: String) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number
      statusCheckRollup {
        contexts(first: 100, after: $endCursor) {
          totalCount
          nodes {
            __typename
            ... on CheckRun { id name status conclusion detailsUrl }
            ... on StatusContext { id context state targetUrl }
          }
          pageInfo { hasNextPage endCursor }
        }
      }
    }
  }
}`;

interface GithubAdapterContext {
	exec: ForgeExec;
	cwd: string;
}

interface CoreArtifact {
	number: number;
	url: string;
	state: string;
	author: ForgeActor;
	createdAt: string;
	updatedAt: string;
	labels: string[];
	assignees: ForgeActor[];
	title: string;
	body: string;
	comments: ForgeComment[];
}

interface PrCore extends CoreArtifact {
	draft: boolean;
	mergeable: "mergeable" | "conflicting" | "unknown";
	reviewDecision: { capability: "supported"; value: string | null };
	head: string;
	base: string;
	changedFiles: number;
}

interface ParsedConnection<T> {
	totalCount: number;
	nodes: T[];
	hasNextPage: boolean;
	endCursor: string | null;
}

function malformed(label: string): never {
	throw new Error(`GitHub ${label} response was malformed or incomplete`);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function record(value: unknown, label: string): Record<string, unknown> {
	if (!isRecord(value)) return malformed(label);
	return value;
}

function string(value: unknown, label: string): string {
	if (typeof value !== "string") return malformed(label);
	return value;
}

function nullableString(value: unknown, label: string): string | null {
	if (value === null) return null;
	return string(value, label);
}

function positiveInteger(value: unknown, label: string): number {
	if (!Number.isSafeInteger(value) || (value as number) <= 0) return malformed(label);
	return value as number;
}

function nonnegativeInteger(value: unknown, label: string): number {
	if (!Number.isSafeInteger(value) || (value as number) < 0) return malformed(label);
	return value as number;
}

function boolean(value: unknown, label: string): boolean {
	if (typeof value !== "boolean") return malformed(label);
	return value;
}

function array(value: unknown, label: string): unknown[] {
	if (!Array.isArray(value)) return malformed(label);
	return value;
}

function parseJson(stdout: string, label: string): unknown {
	try {
		return JSON.parse(stdout);
	} catch {
		return malformed(label);
	}
}

function parsePages(stdout: string, label: string): Record<string, unknown>[] {
	const pages = array(parseJson(stdout, label), label);
	if (pages.length === 0) return malformed(label);
	return pages.map((page) => {
		const parsed = record(page, label);
		if ("errors" in parsed) return malformed(label);
		return parsed;
	});
}

function repositoryParts(repository: ForgeRepository): [owner: string, name: string] {
	if (repository.forge !== "github" || repository.host !== "github.com") {
		throw new Error("GitHub adapter requires a GitHub repository");
	}
	const parts = repository.projectPath.split("/");
	if (parts.length !== 2 || parts.some((part) => !/^[A-Za-z0-9._-]+$/.test(part))) {
		throw new Error("GitHub adapter requires a valid GitHub repository");
	}
	return [parts[0], parts[1]];
}

function graphqlArgs(repository: ForgeRepository, number: number, query: string, paginate = true): string[] {
	const [owner, name] = repositoryParts(repository);
	positiveInteger(number, "artifact number");
	return [
		"api",
		"graphql",
		...(paginate ? ["--paginate", "--slurp"] : []),
		"-f",
		`query=${query}`,
		"-f",
		`owner=${owner}`,
		"-f",
		`name=${name}`,
		"-F",
		`number=${number}`,
	];
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

async function graphqlPages(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	query: string,
	label: string,
	signal?: AbortSignal,
): Promise<Record<string, unknown>[]> {
	return parsePages(await githubCommand(context, graphqlArgs(repository, number, query), signal), label);
}

function pageArtifact(
	page: Record<string, unknown>,
	field: "issue" | "pullRequest",
	label: string,
): Record<string, unknown> {
	const data = record(page.data, label);
	const repository = record(data.repository, label);
	return record(repository[field], label);
}

function parseActor(value: unknown, label: string): ForgeActor {
	if (value === null) return { login: null, kind: "deleted" };
	const actor = record(value, label);
	const login = nullableString(actor.login, label);
	if (login === null) return { login: null, kind: "deleted" };
	return { login, kind: actor.__typename === "Bot" ? "bot" : "user" };
}

function parseLabels(value: unknown, label: string): string[] {
	const connection = record(value, label);
	const totalCount = nonnegativeInteger(connection.totalCount, label);
	const labels = array(connection.nodes, label).map((node) => string(record(node, label).name, label));
	if (labels.length !== totalCount || new Set(labels).size !== labels.length) return malformed(label);
	return labels;
}

function parseAssignees(value: unknown, label: string): ForgeActor[] {
	const connection = record(value, label);
	const totalCount = nonnegativeInteger(connection.totalCount, label);
	const assignees = array(connection.nodes, label).map((node) => parseActor(node, label));
	if (assignees.length !== totalCount || new Set(assignees.map((actor) => actor.login)).size !== assignees.length) {
		return malformed(label);
	}
	return assignees;
}

function parseComment(value: unknown): ForgeComment {
	const comment = record(value, "comments");
	const rawId = comment.databaseId;
	const id =
		typeof rawId === "string" && /^[1-9]\d*$/.test(rawId)
			? rawId
			: Number.isSafeInteger(rawId) && (rawId as number) > 0
				? String(rawId)
				: malformed("comments");
	return {
		id,
		url: string(comment.url, "comments"),
		author: parseActor(comment.author, "comments"),
		body: string(comment.body, "comments"),
		createdAt: string(comment.createdAt, "comments"),
		updatedAt: string(comment.updatedAt, "comments"),
	};
}

function parseConnection<T>(value: unknown, label: string, parseNode: (node: unknown) => T): ParsedConnection<T> {
	const connection = record(value, label);
	const pageInfo = record(connection.pageInfo, label);
	return {
		totalCount: nonnegativeInteger(connection.totalCount, label),
		nodes: array(connection.nodes, label).map(parseNode),
		hasNextPage: boolean(pageInfo.hasNextPage, label),
		endCursor: nullableString(pageInfo.endCursor, label),
	};
}

function collectConnection<T>(
	pages: Record<string, unknown>[],
	connectionFor: (page: Record<string, unknown>) => unknown,
	label: string,
	parseNode: (node: unknown) => T,
	keyFor: (node: T) => string,
): T[] {
	let expectedTotal: number | undefined;
	const cursors = new Set<string>();
	const seen = new Set<string>();
	const nodes: T[] = [];
	for (let index = 0; index < pages.length; index++) {
		const connection = parseConnection(connectionFor(pages[index]), label, parseNode);
		expectedTotal ??= connection.totalCount;
		if (connection.totalCount !== expectedTotal) return malformed(label);
		const final = index === pages.length - 1;
		if (connection.hasNextPage === final) return malformed(label);
		if (connection.hasNextPage && (connection.endCursor === null || cursors.has(connection.endCursor))) {
			return malformed(label);
		}
		if (connection.endCursor !== null) cursors.add(connection.endCursor);
		for (const node of connection.nodes) {
			const key = keyFor(node);
			if (seen.has(key)) return malformed(label);
			seen.add(key);
			nodes.push(node);
		}
	}
	if (nodes.length !== expectedTotal) return malformed(label);
	return nodes;
}

function parseCore(value: Record<string, unknown>, label: string): Omit<CoreArtifact, "comments"> {
	return {
		number: positiveInteger(value.number, label),
		url: string(value.url, label),
		state: string(value.state, label).toLowerCase(),
		author: parseActor(value.author, label),
		createdAt: string(value.createdAt, label),
		updatedAt: string(value.updatedAt, label),
		labels: parseLabels(value.labels, label),
		assignees: parseAssignees(value.assignees, label),
		title: string(value.title, label),
		body: string(value.body, label),
	};
}

function stableCore(pages: Record<string, unknown>[], field: "issue" | "pullRequest", label: string) {
	const first = parseCore(pageArtifact(pages[0], field, label), label);
	for (const page of pages.slice(1)) {
		if (JSON.stringify(parseCore(pageArtifact(page, field, label), label)) !== JSON.stringify(first)) {
			return malformed(label);
		}
	}
	return first;
}

function commentsFrom(pages: Record<string, unknown>[], field: "issue" | "pullRequest"): ForgeComment[] {
	return collectConnection(
		pages,
		(page) => pageArtifact(page, field, "comments").comments,
		"comments",
		parseComment,
		(comment) => comment.id,
	);
}

async function readCore(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	signal?: AbortSignal,
): Promise<CoreArtifact | PrCore> {
	const field = kind === "issue" ? "issue" : "pullRequest";
	const pages = await graphqlPages(
		context,
		repository,
		number,
		kind === "issue" ? ISSUE_QUERY : PR_QUERY,
		kind === "issue" ? "issue" : "pull request",
		signal,
	);
	const core = stableCore(pages, field, kind === "issue" ? "issue" : "pull request");
	const comments = commentsFrom(pages, field);
	if (core.number !== number) return malformed(kind === "issue" ? "issue" : "pull request");
	if (kind === "issue") return { ...core, comments };
	const value = pageArtifact(pages[0], field, "pull request");
	const rawMergeable = string(value.mergeable, "pull request").toLowerCase();
	const mergeable =
		rawMergeable === "mergeable" || rawMergeable === "conflicting" || rawMergeable === "unknown"
			? rawMergeable
			: malformed("pull request");
	for (const page of pages.slice(1)) {
		const current = pageArtifact(page, field, "pull request");
		for (const key of [
			"isDraft",
			"mergeable",
			"reviewDecision",
			"headRefName",
			"baseRefName",
			"changedFiles",
		] as const) {
			if (current[key] !== value[key]) return malformed("pull request");
		}
	}
	return {
		...core,
		comments,
		draft: boolean(value.isDraft, "pull request"),
		mergeable,
		reviewDecision: {
			capability: "supported",
			value: nullableString(value.reviewDecision, "pull request")?.toLowerCase() ?? null,
		},
		head: string(value.headRefName, "pull request"),
		base: string(value.baseRefName, "pull request"),
		changedFiles: nonnegativeInteger(value.changedFiles, "pull request"),
	};
}

function repositoriesEqual(left: ForgeRepository, right: ForgeRepository): boolean {
	return (
		left.forge === right.forge &&
		left.host === right.host &&
		left.projectPath.toLowerCase() === right.projectPath.toLowerCase()
	);
}

function githubArtifactRepository(
	url: string,
	kind: ForgeArtifactKind,
	number: number,
	label: string,
): ForgeRepository {
	const segment = kind === "issue" ? "issues" : "pull";
	const match = new RegExp(`^https://github\\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/${segment}/${number}$`).exec(
		url,
	);
	if (match === null) return malformed(label);
	return { forge: "github", host: "github.com", projectPath: `${match[1]}/${match[2]}` };
}

function parseRelationship(value: unknown, relation: "parent" | "child", source: "native" | "task-list") {
	const issue = record(value, "relationships");
	const number = positiveInteger(issue.number, "relationships");
	const url = string(issue.url, "relationships");
	return {
		repository: githubArtifactRepository(url, "issue", number, "relationships"),
		relation,
		source,
		number,
		url,
		state: string(issue.state, "relationships").toLowerCase(),
		title: string(issue.title, "relationships"),
	} satisfies ForgeIssueRelationship;
}

async function readNativeRelationships(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	signal?: AbortSignal,
): Promise<ForgeIssueRelationship[]> {
	const pages = await graphqlPages(context, repository, number, RELATIONSHIPS_QUERY, "relationships", signal);
	const first = pageArtifact(pages[0], "issue", "relationships");
	if (positiveInteger(first.number, "relationships") !== number) return malformed("relationships");
	const parent = first.parent === null ? null : parseRelationship(first.parent, "parent", "native");
	for (const page of pages.slice(1)) {
		const issue = pageArtifact(page, "issue", "relationships");
		if (positiveInteger(issue.number, "relationships") !== number) return malformed("relationships");
		const currentParent = issue.parent === null ? null : parseRelationship(issue.parent, "parent", "native");
		if (JSON.stringify(currentParent) !== JSON.stringify(parent)) return malformed("relationships");
	}
	const children = collectConnection(
		pages,
		(page) => pageArtifact(page, "issue", "relationships").subIssues,
		"relationships",
		(node) => parseRelationship(node, "child", "native"),
		(child) => `${child.repository.projectPath.toLowerCase()}:${child.number}`,
	);
	const relationships = parent === null ? children : [parent, ...children];
	if (
		new Set(
			relationships.map((item) => `${item.relation}:${item.repository.projectPath.toLowerCase()}:${item.number}`),
		).size !== relationships.length
	) {
		return malformed("relationships");
	}
	return relationships;
}

function taskListNumbers(body: string, parentNumber: number): number[] {
	const numbers = new Set<number>();
	for (const line of body.split(/\r?\n/)) {
		const task = /^\s*[-*+]\s+\[[ xX]\]\s+(.+)$/.exec(line);
		if (!task) continue;
		for (const match of task[1].matchAll(/(^|[^A-Za-z0-9_./-])#([1-9]\d*)/g)) {
			const prefix = task[1].slice(0, match.index);
			if (/(?:related to|blocked by|see also|depends on)\b/i.test(prefix)) continue;
			const number = Number(match[2]);
			if (Number.isSafeInteger(number) && number !== parentNumber) numbers.add(number);
		}
	}
	return [...numbers];
}

async function readTaskListRelationships(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	parentNumber: number,
	body: string,
	signal?: AbortSignal,
): Promise<ForgeIssueRelationship[]> {
	const relationships: ForgeIssueRelationship[] = [];
	for (const number of taskListNumbers(body, parentNumber)) {
		const stdout = await githubCommand(
			context,
			graphqlArgs(repository, number, TASK_LIST_ISSUE_QUERY, false),
			signal,
		);
		const response = record(parseJson(stdout, "relationships"), "relationships");
		if ("errors" in response) return malformed("relationships");
		const data = record(response.data, "relationships");
		const remoteRepository = record(data.repository, "relationships");
		const relationship = parseRelationship(remoteRepository.task, "child", "task-list");
		if (relationship.number !== number || !repositoriesEqual(relationship.repository, repository)) {
			return malformed("relationships");
		}
		relationships.push(relationship);
	}
	return relationships;
}

function nativeRelationshipsUnsupported(error: ForgeCommandError): boolean {
	if (error.kind !== "failed" || error.invocation.process === undefined) return false;
	let pages: unknown;
	try {
		pages = JSON.parse(error.invocation.process.stdout);
	} catch {
		return false;
	}
	if (!Array.isArray(pages) || pages.length === 0) return false;
	return pages.every((page) => {
		if (!isRecord(page) || !Array.isArray(page.errors) || page.errors.length === 0) return false;
		return page.errors.every((item) => {
			if (!isRecord(item) || !isRecord(item.extensions)) return false;
			const extensions = item.extensions;
			return (
				extensions.code === "undefinedField" &&
				extensions.typeName === "Issue" &&
				(extensions.fieldName === "parent" || extensions.fieldName === "subIssues")
			);
		});
	});
}

async function readRelationships(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	body: string,
	signal?: AbortSignal,
): Promise<ForgeIssueRelationship[]> {
	try {
		return await readNativeRelationships(context, repository, number, signal);
	} catch (error) {
		if (!(error instanceof ForgeCommandError) || !nativeRelationshipsUnsupported(error) || signal?.aborted) {
			throw error;
		}
		return readTaskListRelationships(context, repository, number, body, signal);
	}
}

function parseFile(value: unknown): ForgePrFile {
	const file = record(value, "files");
	return {
		path: string(file.filename, "files"),
		status: string(file.status, "files").toLowerCase(),
		additions: nonnegativeInteger(file.additions, "files"),
		deletions: nonnegativeInteger(file.deletions, "files"),
		previousPath: file.previous_filename === undefined ? null : string(file.previous_filename, "files"),
	};
}

async function readFiles(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	expectedCount: number,
	signal?: AbortSignal,
): Promise<ForgePrFile[]> {
	repositoryParts(repository);
	const pages = array(
		parseJson(
			await githubCommand(
				context,
				["api", "--paginate", "--slurp", `repos/${repository.projectPath}/pulls/${number}/files?per_page=100`],
				signal,
			),
			"files",
		),
		"files",
	);
	if (pages.length === 0) return malformed("files");
	const files = pages.flatMap((page) => array(page, "files").map(parseFile));
	if (files.length !== expectedCount || new Set(files.map((file) => file.path)).size !== files.length) {
		return malformed("files");
	}
	return files;
}

function parseCommit(value: unknown): ForgePrCommit {
	const commit = record(record(value, "commits").commit, "commits");
	const author = commit.author === null ? null : string(record(commit.author, "commits").name, "commits");
	return {
		sha: string(commit.oid, "commits"),
		title: string(commit.messageHeadline, "commits"),
		author,
		createdAt: string(commit.authoredDate, "commits"),
		url: nullableString(commit.url, "commits"),
	};
}

async function readCommits(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	signal?: AbortSignal,
): Promise<ForgePrCommit[]> {
	const pages = await graphqlPages(context, repository, number, COMMITS_QUERY, "commits", signal);
	for (const page of pages) {
		if (positiveInteger(pageArtifact(page, "pullRequest", "commits").number, "commits") !== number) {
			return malformed("commits");
		}
	}
	return collectConnection(
		pages,
		(page) => pageArtifact(page, "pullRequest", "commits").commits,
		"commits",
		parseCommit,
		(commit) => commit.sha,
	);
}

function parseCheck(value: unknown): ForgePrCheck {
	const check = record(value, "checks");
	const type = string(check.__typename, "checks");
	if (type === "CheckRun") {
		return {
			id: string(check.id, "checks"),
			name: string(check.name, "checks"),
			status: string(check.status, "checks").toLowerCase(),
			conclusion: nullableString(check.conclusion, "checks")?.toLowerCase() ?? null,
			url: nullableString(check.detailsUrl, "checks"),
		};
	}
	if (type === "StatusContext") {
		const state = string(check.state, "checks").toLowerCase();
		const pending = state === "pending" || state === "expected";
		return {
			id: string(check.id, "checks"),
			name: string(check.context, "checks"),
			status: pending ? "pending" : "completed",
			conclusion: pending ? null : state,
			url: nullableString(check.targetUrl, "checks"),
		};
	}
	return malformed("checks");
}

async function readChecks(
	context: GithubAdapterContext,
	repository: ForgeRepository,
	number: number,
	signal?: AbortSignal,
): Promise<ForgePrCheck[]> {
	const pages = await graphqlPages(context, repository, number, CHECKS_QUERY, "checks", signal);
	for (const page of pages) {
		if (positiveInteger(pageArtifact(page, "pullRequest", "checks").number, "checks") !== number) {
			return malformed("checks");
		}
	}
	const first = pageArtifact(pages[0], "pullRequest", "checks");
	if (first.statusCheckRollup === null) {
		if (pages.length !== 1) return malformed("checks");
		return [];
	}
	return collectConnection(
		pages,
		(page) => record(pageArtifact(page, "pullRequest", "checks").statusCheckRollup, "checks").contexts,
		"checks",
		parseCheck,
		(check) => check.id,
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
	const responseRepository = githubArtifactRepository(url, kind, number, "mutation response");
	if (
		(expectedNumber !== undefined && number !== expectedNumber) ||
		!repositoriesEqual(responseRepository, repository)
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
	const rawId = response.id;
	const id =
		typeof rawId === "string" && /^[1-9]\d*$/.test(rawId)
			? rawId
			: Number.isSafeInteger(rawId) && (rawId as number) > 0
				? String(rawId)
				: malformed("mutation response");
	const url = string(response.html_url, "mutation response");
	const issueUrl = string(response.issue_url, "mutation response");
	const segment = kind === "issue" ? "issues" : "pull";
	const commentMatch = new RegExp(
		`^https://github\\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/${segment}/${parentNumber}#issuecomment-${id}$`,
	).exec(url);
	const issueMatch = new RegExp(
		`^https://api\\.github\\.com/repos/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/issues/${parentNumber}$`,
	).exec(issueUrl);
	if (commentMatch === null || issueMatch === null) return malformed("mutation response");
	const commentRepository: ForgeRepository = {
		forge: "github",
		host: "github.com",
		projectPath: `${commentMatch[1]}/${commentMatch[2]}`,
	};
	const issueRepository: ForgeRepository = {
		forge: "github",
		host: "github.com",
		projectPath: `${issueMatch[1]}/${issueMatch[2]}`,
	};
	if (
		(expectedId !== undefined && id !== expectedId) ||
		!repositoriesEqual(commentRepository, repository) ||
		!repositoriesEqual(issueRepository, repository)
	) {
		return malformed("mutation response");
	}
	return { kind: "comment", id, url };
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
	const stdout = await githubCommand(
		context,
		["api", "--method", method, "--input", "-", endpoint],
		signal,
		JSON.stringify(payload),
	);
	return parseJson(stdout, "mutation response");
}

export function createGithubAdapter(exec: ForgeExec, cwd: string): ForgeAdapter {
	const context = { exec, cwd };
	return {
		async readArtifact(repository, kind, number, include, signal): Promise<ForgeArtifact> {
			repositoryParts(repository);
			positiveInteger(number, "artifact number");
			const requested = new Set<ForgePrSection>(include);
			if (
				requested.size !== include.length ||
				[...requested].some((section) => !["files", "commits", "checks"].includes(section))
			) {
				throw new Error("GitHub adapter received invalid PR sections");
			}
			if (kind === "issue") {
				if (requested.size !== 0) throw new Error("Issue reads do not support PR sections");
				const core = (await readCore(context, repository, kind, number, signal)) as CoreArtifact;
				const relationships = await readRelationships(context, repository, number, core.body, signal);
				return { kind, ...core, relationships: { capability: "supported", items: relationships } };
			}
			const core = (await readCore(context, repository, kind, number, signal)) as PrCore;
			const sections: {
				files?: ForgePrFile[];
				commits?: ForgePrCommit[];
				checks?: ForgePrCheck[];
			} = {};
			if (requested.has("files"))
				sections.files = await readFiles(context, repository, number, core.changedFiles, signal);
			if (requested.has("commits")) sections.commits = await readCommits(context, repository, number, signal);
			if (requested.has("checks")) sections.checks = await readChecks(context, repository, number, signal);
			const { draft, mergeable, reviewDecision, head, base, changedFiles: _changedFiles, ...artifact } = core;
			return {
				kind,
				...artifact,
				readiness: { draft, mergeable, reviewDecision, head, base },
				sections,
			};
		},

		async createArtifact(repository, input: ForgeCreateInput, signal) {
			const endpoint = `repos/${repository.projectPath}/${input.kind === "issue" ? "issues" : "pulls"}`;
			const payload =
				input.kind === "issue"
					? { title: input.title, body: input.body }
					: {
							title: input.title,
							body: input.body,
							head: input.head,
							base: input.base,
							draft: input.draft,
						};
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
			positiveInteger(input.number, "artifact number");
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
			positiveInteger(input.number, "artifact number");
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
