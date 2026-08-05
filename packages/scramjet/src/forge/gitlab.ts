import { FORGE_EXEC_TIMEOUT_MS, type ForgeExec, runForgeCommand } from "./client.js";
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
	ForgeIssueRelationships,
	ForgePrCheck,
	ForgePrCommit,
	ForgePrFile,
	ForgePrSection,
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

interface GitlabCore {
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
	projectId: number;
	globalId: number;
	noteCount: number;
}

interface GitlabPrCore extends GitlabCore {
	draft: boolean;
	mergeable: "mergeable" | "conflicting" | "unknown";
	reviewDecision: { capability: "unsupported" };
	head: string;
	base: string;
	changedFiles: number | "pending" | "overflow";
}

interface ParsedHierarchyPage {
	parent: ForgeIssueRelationship | null;
	children: ForgeIssueRelationship[];
	count: number;
	hasNextPage: boolean;
	endCursor: string | null;
}

function malformed(label: string): never {
	throw new Error(`GitLab ${label} response was malformed or incomplete`);
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

function boolean(value: unknown, label: string): boolean {
	if (typeof value !== "boolean") return malformed(label);
	return value;
}

function array(value: unknown, label: string): unknown[] {
	if (!Array.isArray(value)) return malformed(label);
	return value;
}

function positiveInteger(value: unknown, label: string): number {
	if (!Number.isSafeInteger(value) || (value as number) <= 0) return malformed(label);
	return value as number;
}

function nonnegativeInteger(value: unknown, label: string): number {
	if (!Number.isSafeInteger(value) || (value as number) < 0) return malformed(label);
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

function parseNdjsonValues(stdout: string, label: string): unknown[] {
	return stdout
		.split(/\r?\n/)
		.filter((line) => line.length > 0)
		.map((line) => parseJson(line, label));
}

function parseNdjson(stdout: string, label: string): Record<string, unknown>[] {
	const pages = parseNdjsonValues(stdout, label).map((value) => record(value, label));
	if (pages.length === 0) return malformed(label);
	if (pages.some((page) => "errors" in page)) return malformed(label);
	return pages;
}

function normalizeState(value: unknown, label: string): string {
	const state = string(value, label).toLowerCase();
	return state === "opened" ? "open" : state;
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

function artifactUrl(repository: ForgeRepository, kind: ForgeArtifactKind, number: number): string {
	return `https://gitlab.com/${repository.projectPath}/-/${kind === "issue" ? "work_items" : "merge_requests"}/${number}`;
}

function artifactPath(kind: ForgeArtifactKind): "issues" | "merge_requests" {
	return kind === "issue" ? "issues" : "merge_requests";
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

async function readObject(
	context: GitlabAdapterContext,
	endpoint: string,
	label: string,
	signal?: AbortSignal,
): Promise<Record<string, unknown>> {
	return record(parseJson(await gitlabCommand(context, ["api", endpoint], signal), label), label);
}

async function readList(
	context: GitlabAdapterContext,
	endpoint: string,
	label: string,
	signal?: AbortSignal,
): Promise<unknown[]> {
	return parseNdjsonValues(
		await gitlabCommand(context, ["api", "--paginate", "--output", "ndjson", endpoint], signal),
		label,
	);
}

function authoritativeTotal(headers: string, label: string): number {
	const lines = headers.split(/\r?\n/).filter((line) => line.trim() !== "");
	const statuses = lines.filter((line) => /^HTTP\/\S+\s+\d{3}(?:\s|$)/i.test(line));
	if (statuses.length !== 1 || !/^HTTP\/\S+\s+2\d{2}(?:\s|$)/i.test(statuses[0])) return malformed(label);
	const totals = lines.flatMap((line) => {
		const match = /^x-total:\s*(\d+)\s*$/i.exec(line);
		return match === null ? [] : [match[1]];
	});
	if (totals.length !== 1) return malformed(label);
	const total = Number(totals[0]);
	if (!Number.isSafeInteger(total)) return malformed(label);
	return total;
}

async function readCountedList(
	context: GitlabAdapterContext,
	endpoint: string,
	label: string,
	signal?: AbortSignal,
): Promise<unknown[]> {
	const items = await readList(context, endpoint, label, signal);
	const headers = await gitlabCommand(context, ["api", "--include", "--silent", endpoint], signal);
	if (items.length !== authoritativeTotal(headers, label)) return malformed(label);
	return items;
}

function parseActor(value: unknown, label: string): ForgeActor {
	if (value === null) return { login: null, kind: "deleted" };
	const actor = record(value, label);
	const login = string(actor.username, label);
	const bot = actor.bot === undefined ? false : boolean(actor.bot, label);
	return { login, kind: bot ? "bot" : "user" };
}

function parseLabels(value: unknown, label: string): string[] {
	const labels = array(value, label).map((item) => string(item, label));
	if (new Set(labels).size !== labels.length) return malformed(label);
	return labels;
}

function parseAssignees(value: unknown, label: string): ForgeActor[] {
	const assignees = array(value, label).map((item) => parseActor(item, label));
	if (new Set(assignees.map((actor) => actor.login)).size !== assignees.length) return malformed(label);
	return assignees;
}

function parseChangedFiles(value: unknown): number | "pending" | "overflow" {
	if (value === null || value === "") return "pending";
	const count = string(value, "merge request");
	if (/^\d+$/.test(count)) {
		const number = Number(count);
		if (Number.isSafeInteger(number)) return number;
	}
	if (/^[1-9]\d*\+$/.test(count)) return "overflow";
	return malformed("merge request");
}

function parseCore(
	value: Record<string, unknown>,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
): Omit<GitlabCore, "comments"> | Omit<GitlabPrCore, "comments"> {
	const label = kind === "issue" ? "issue" : "merge request";
	const iid = positiveInteger(value.iid, label);
	const url = string(value.web_url, label);
	if (iid !== number || url !== artifactUrl(repository, kind, number)) return malformed(label);
	const common = {
		number,
		url,
		state: normalizeState(value.state, label),
		author: parseActor(value.author, label),
		createdAt: string(value.created_at, label),
		updatedAt: string(value.updated_at, label),
		labels: parseLabels(value.labels, label),
		assignees: parseAssignees(value.assignees, label),
		title: string(value.title, label),
		body: nullableString(value.description, label) ?? "",
		projectId: positiveInteger(value.project_id, label),
		globalId: positiveInteger(value.id, label),
		noteCount: nonnegativeInteger(value.user_notes_count, label),
	};
	if (kind === "issue") return common;
	const detailedStatus = string(value.detailed_merge_status, label).toLowerCase();
	const conflicts = boolean(value.has_conflicts, label);
	const mergeable =
		conflicts || detailedStatus === "conflict"
			? "conflicting"
			: detailedStatus === "mergeable"
				? "mergeable"
				: "unknown";
	return {
		...common,
		draft: boolean(value.draft, label),
		mergeable,
		reviewDecision: { capability: "unsupported" },
		head: string(value.source_branch, label),
		base: string(value.target_branch, label),
		changedFiles: parseChangedFiles(value.changes_count),
	};
}

function parseNote(
	value: unknown,
	core: Omit<GitlabCore, "comments"> | Omit<GitlabPrCore, "comments">,
	kind: ForgeArtifactKind,
): {
	id: string;
	system: boolean;
	type: string | null;
	position: Record<string, unknown> | null;
	comment: ForgeComment;
} {
	const note = record(value, "notes");
	const id = decimalId(note.id, "notes");
	const system = boolean(note.system, "notes");
	const type = nullableString(note.type, "notes");
	const position = note.position === null ? null : record(note.position, "notes");
	if (
		positiveInteger(note.project_id, "notes") !== core.projectId ||
		positiveInteger(note.noteable_id, "notes") !== core.globalId ||
		positiveInteger(note.noteable_iid, "notes") !== core.number ||
		string(note.noteable_type, "notes") !== (kind === "issue" ? "Issue" : "MergeRequest")
	) {
		return malformed("notes");
	}
	return {
		id,
		system,
		type,
		position,
		comment: {
			id,
			url: `${core.url}#note_${id}`,
			author: parseActor(note.author, "notes"),
			body: string(note.body, "notes"),
			createdAt: string(note.created_at, "notes"),
			updatedAt: string(note.updated_at, "notes"),
		},
	};
}

async function readComments(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	core: Omit<GitlabCore, "comments"> | Omit<GitlabPrCore, "comments">,
	signal?: AbortSignal,
): Promise<ForgeComment[]> {
	const notes = (
		await readList(
			context,
			`projects/${projectId(repository)}/${artifactPath(kind)}/${core.number}/notes?per_page=100&sort=asc&order_by=created_at`,
			"notes",
			signal,
		)
	).map((note) => parseNote(note, core, kind));
	if (
		new Set(notes.map((note) => note.id)).size !== notes.length ||
		notes.filter((note) => !note.system).length !== core.noteCount
	) {
		return malformed("notes");
	}
	return notes
		.filter((note) => !note.system && note.position === null && (note.type === null || note.type === "Note"))
		.map((note) => note.comment);
}

async function readCore(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	kind: ForgeArtifactKind,
	number: number,
	signal?: AbortSignal,
): Promise<GitlabCore | GitlabPrCore> {
	const label = kind === "issue" ? "issue" : "merge request";
	const raw = await readObject(
		context,
		`projects/${projectId(repository)}/${artifactPath(kind)}/${number}`,
		label,
		signal,
	);
	const core = parseCore(raw, repository, kind, number);
	const comments = await readComments(context, repository, kind, core, signal);
	return { ...core, comments };
}

function relationshipRepository(url: string, number: number): ForgeRepository {
	const match = new RegExp(
		`^https://gitlab\\.com/([A-Za-z0-9._-]+(?:/[A-Za-z0-9._-]+)+)/-/(?:issues|work_items)/${number}$`,
	).exec(url);
	if (match === null) return malformed("relationships");
	return { forge: "gitlab", host: "gitlab.com", projectPath: match[1] };
}

function parseRelationship(value: unknown, relation: "parent" | "child"): ForgeIssueRelationship {
	const item = record(value, "relationships");
	const number = Number(decimalId(item.iid, "relationships"));
	const url = string(item.webUrl, "relationships");
	return {
		repository: relationshipRepository(url, number),
		relation,
		source: "native",
		number,
		url,
		state: normalizeState(item.state, "relationships"),
		title: string(item.title, "relationships"),
	};
}

function hierarchyPage(value: Record<string, unknown>, number: number): ParsedHierarchyPage | null {
	const data = record(value.data, "relationships");
	const project = record(data.project, "relationships");
	const workItems = record(project.workItems, "relationships");
	if (nonnegativeInteger(workItems.count, "relationships") !== 1) return malformed("relationships");
	const nodes = array(workItems.nodes, "relationships");
	if (nodes.length !== 1) return malformed("relationships");
	const workItem = record(nodes[0], "relationships");
	if (Number(decimalId(workItem.iid, "relationships")) !== number) return malformed("relationships");
	const widgets = array(workItem.widgets, "relationships");
	if (widgets.length === 0) return null;
	if (widgets.length !== 1) return malformed("relationships");
	const hierarchy = record(widgets[0], "relationships");
	const parent = hierarchy.parent === null ? null : parseRelationship(hierarchy.parent, "parent");
	const children = record(hierarchy.children, "relationships");
	const pageInfo = record(children.pageInfo, "relationships");
	return {
		parent,
		children: array(children.nodes, "relationships").map((child) => parseRelationship(child, "child")),
		count: nonnegativeInteger(children.count, "relationships"),
		hasNextPage: boolean(pageInfo.hasNextPage, "relationships"),
		endCursor: nullableString(pageInfo.endCursor, "relationships"),
	};
}

async function readRelationships(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	number: number,
	signal?: AbortSignal,
): Promise<ForgeIssueRelationships> {
	const stdout = await gitlabCommand(
		context,
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
		signal,
	);
	const parsed = parseNdjson(stdout, "relationships").map((page) => hierarchyPage(page, number));
	if (parsed.every((page) => page === null)) {
		if (parsed.length !== 1) return malformed("relationships");
		return { capability: "unsupported", items: [] };
	}
	if (parsed.some((page) => page === null)) return malformed("relationships");
	const pages = parsed as ParsedHierarchyPage[];
	const parent = pages[0].parent;
	const expected = pages[0].count;
	const cursors = new Set<string>();
	const seen = new Set<string>();
	const children: ForgeIssueRelationship[] = [];
	for (let index = 0; index < pages.length; index++) {
		const page = pages[index];
		if (page.count !== expected || JSON.stringify(page.parent) !== JSON.stringify(parent))
			return malformed("relationships");
		const final = index === pages.length - 1;
		if (page.hasNextPage === final) return malformed("relationships");
		if (page.hasNextPage && (page.endCursor === null || cursors.has(page.endCursor)))
			return malformed("relationships");
		if (page.endCursor !== null) cursors.add(page.endCursor);
		for (const child of page.children) {
			const key = `${child.repository.projectPath}:${child.number}`;
			if (seen.has(key)) return malformed("relationships");
			seen.add(key);
			children.push(child);
		}
	}
	if (children.length !== expected) return malformed("relationships");
	const items = parent === null ? children : [parent, ...children];
	if (
		new Set(items.map((item) => `${item.relation}:${item.repository.projectPath}:${item.number}`)).size !==
		items.length
	) {
		return malformed("relationships");
	}
	return { capability: "supported", items };
}

function diffCounts(diff: string): { additions: number; deletions: number } {
	let additions = 0;
	let deletions = 0;
	let inHunk = false;
	for (const line of diff.split("\n")) {
		if (line.startsWith("@@")) {
			inHunk = true;
			continue;
		}
		if (!inHunk) continue;
		if (line.startsWith("+")) additions++;
		if (line.startsWith("-")) deletions++;
	}
	return { additions, deletions };
}

function parseFile(value: unknown): ForgePrFile {
	const file = record(value, "files");
	if (boolean(file.collapsed, "files") || boolean(file.too_large, "files")) return malformed("files");
	const added = boolean(file.new_file, "files");
	const renamed = boolean(file.renamed_file, "files");
	const deleted = boolean(file.deleted_file, "files");
	if ([added, renamed, deleted].filter(Boolean).length > 1) return malformed("files");
	const path = string(file.new_path, "files");
	const oldPath = string(file.old_path, "files");
	return {
		path,
		status: added ? "added" : renamed ? "renamed" : deleted ? "deleted" : "modified",
		...diffCounts(string(file.diff, "files")),
		previousPath: renamed ? oldPath : null,
	};
}

async function readFiles(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	number: number,
	expectedCount: number,
	signal?: AbortSignal,
): Promise<ForgePrFile[]> {
	const files = (
		await readList(
			context,
			`projects/${projectId(repository)}/merge_requests/${number}/diffs?per_page=100`,
			"files",
			signal,
		)
	).map(parseFile);
	if (files.length !== expectedCount || new Set(files.map((file) => file.path)).size !== files.length) {
		return malformed("files");
	}
	return files;
}

function parseCommit(value: unknown): ForgePrCommit {
	const commit = record(value, "commits");
	return {
		sha: string(commit.id, "commits"),
		title: string(commit.title, "commits"),
		author: nullableString(commit.author_name, "commits"),
		createdAt: string(commit.authored_date, "commits"),
		url: nullableString(commit.web_url, "commits"),
	};
}

async function readCommits(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	number: number,
	signal?: AbortSignal,
): Promise<ForgePrCommit[]> {
	const commits = (
		await readCountedList(
			context,
			`projects/${projectId(repository)}/merge_requests/${number}/commits?per_page=100`,
			"commits",
			signal,
		)
	).map(parseCommit);
	if (new Set(commits.map((commit) => commit.sha)).size !== commits.length) return malformed("commits");
	return commits;
}

function parseCheck(value: unknown, expectedProjectId: number): ForgePrCheck {
	const pipeline = record(value, "checks");
	const id = decimalId(pipeline.id, "checks");
	if (positiveInteger(pipeline.project_id, "checks") !== expectedProjectId) return malformed("checks");
	const name = nullableString(pipeline.name, "checks");
	const status = string(pipeline.status, "checks").toLowerCase();
	const terminal = new Set(["success", "failed", "canceled", "cancelled", "skipped"]);
	return {
		id,
		name: name === null || name.length === 0 ? `pipeline #${id}` : name,
		status,
		conclusion: terminal.has(status) ? status : null,
		url: nullableString(pipeline.web_url, "checks"),
	};
}

async function readChecks(
	context: GitlabAdapterContext,
	repository: ForgeRepository,
	number: number,
	expectedProjectId: number,
	signal?: AbortSignal,
): Promise<ForgePrCheck[]> {
	const checks = (
		await readCountedList(
			context,
			`projects/${projectId(repository)}/merge_requests/${number}/pipelines?per_page=100`,
			"checks",
			signal,
		)
	).map((pipeline) => parseCheck(pipeline, expectedProjectId));
	if (new Set(checks.map((check) => check.id)).size !== checks.length) return malformed("checks");
	return checks;
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
	if ((expectedNumber !== undefined && number !== expectedNumber) || url !== artifactUrl(repository, kind, number)) {
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
	return { kind: "comment", id, url: `${artifactUrl(repository, kind, parentNumber)}#note_${id}` };
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

function mergeRequestTitle(title: string, draft: boolean): string {
	const prefixed = /^(?:Draft:|\[Draft\]|\(Draft\))\s*/i.test(title);
	if (!draft && prefixed)
		throw new Error("GitLab cannot create a non-draft merge request with a draft-prefixed title");
	if (draft && !prefixed)
		throw new Error("GitLab draft merge requests require the exact approved title to include a draft prefix");
	return title;
}

export function createGitlabAdapter(exec: ForgeExec, cwd: string): ForgeAdapter {
	const context = { exec, cwd };
	return {
		async readArtifact(repository, kind, number, include, signal): Promise<ForgeArtifact> {
			projectId(repository);
			positiveInteger(number, "artifact number");
			const requested = new Set<ForgePrSection>(include);
			if (
				requested.size !== include.length ||
				[...requested].some((section) => !["files", "commits", "checks"].includes(section))
			) {
				throw new Error("GitLab adapter received invalid PR sections");
			}
			if (kind === "issue") {
				if (requested.size !== 0) throw new Error("Issue reads do not support PR sections");
				const core = (await readCore(context, repository, kind, number, signal)) as GitlabCore;
				const relationships = await readRelationships(context, repository, number, signal);
				const { projectId: _projectId, globalId: _globalId, noteCount: _noteCount, ...artifact } = core;
				return { kind, ...artifact, relationships };
			}
			const core = (await readCore(context, repository, kind, number, signal)) as GitlabPrCore;
			const sections: { files?: ForgePrFile[]; commits?: ForgePrCommit[]; checks?: ForgePrCheck[] } = {};
			if (requested.has("files")) {
				if (typeof core.changedFiles !== "number") return malformed("files");
				sections.files = await readFiles(context, repository, number, core.changedFiles, signal);
			}
			if (requested.has("commits")) sections.commits = await readCommits(context, repository, number, signal);
			if (requested.has("checks")) {
				sections.checks = await readChecks(context, repository, number, core.projectId, signal);
			}
			const {
				draft,
				mergeable,
				reviewDecision,
				head,
				base,
				changedFiles: _changedFiles,
				projectId: _projectId,
				globalId: _globalId,
				noteCount: _noteCount,
				...artifact
			} = core;
			return {
				kind,
				...artifact,
				readiness: { draft, mergeable, reviewDecision, head, base },
				sections,
			};
		},

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
			positiveInteger(input.number, "artifact number");
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
			positiveInteger(input.number, "artifact number");
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
			positiveInteger(input.number, "artifact number");
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
