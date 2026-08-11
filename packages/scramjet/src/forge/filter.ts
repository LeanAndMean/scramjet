import type { ForgeName, ForgeReadSegmentId } from "./types.js";

export type ForgeFilterContext = ForgeReadSegmentId | "user" | "label" | "file" | "commit" | "check";

const COMMON_RECORD_FIELDS = [
	"node_id",
	"url",
	"repository_url",
	"labels_url",
	"comments_url",
	"events_url",
	"timeline_url",
	"statuses_url",
	"commits_url",
	"comments_url",
	"issue_url",
	"pull_request_url",
	"archive_url",
	"assignees_url",
	"blobs_url",
	"branches_url",
	"collaborators_url",
	"compare_url",
	"contents_url",
	"contributors_url",
	"deployments_url",
	"downloads_url",
	"forks_url",
	"git_commits_url",
	"git_refs_url",
	"git_tags_url",
	"hooks_url",
	"keys_url",
	"languages_url",
	"merges_url",
	"milestones_url",
	"notifications_url",
	"releases_url",
	"stargazers_url",
	"subscribers_url",
	"subscription_url",
	"tags_url",
	"teams_url",
	"trees_url",
	"reactions",
	"performed_via_github_app",
] as const;

const USER_FIELDS = [
	"node_id",
	"avatar_url",
	"gravatar_id",
	"url",
	"html_url",
	"followers_url",
	"following_url",
	"gists_url",
	"starred_url",
	"subscriptions_url",
	"organizations_url",
	"repos_url",
	"events_url",
	"received_events_url",
] as const;

export const FORGE_REPLY_DELETE_FIELDS: Readonly<
	Record<ForgeName, Partial<Record<ForgeFilterContext, readonly string[]>>>
> = {
	github: {
		artifact: COMMON_RECORD_FIELDS,
		comments: COMMON_RECORD_FIELDS,
		sub_issues: COMMON_RECORD_FIELDS,
		parent: COMMON_RECORD_FIELDS,
		files: ["blob_url", "raw_url", "contents_url"],
		commits: COMMON_RECORD_FIELDS,
		check_runs: ["node_id", "url", "check_suite", "app", "pull_requests"],
		status: ["url", "avatar_url", "node_id"],
		user: USER_FIELDS,
		label: ["node_id", "url"],
		file: ["blob_url", "raw_url", "contents_url"],
		commit: COMMON_RECORD_FIELDS,
		check: ["node_id", "url", "check_suite", "app", "pull_requests"],
	},
	gitlab: {
		artifact: ["references", "_links", "task_completion_status"],
		comments: ["resolvable", "confidential", "commands_changes"],
		relationships: [],
		files: [],
		commits: ["parent_ids"],
		pipelines: [],
		user: ["avatar_url", "web_url"],
		label: [],
		file: [],
		commit: ["parent_ids"],
		check: [],
	},
};

function nestedContext(key: string, current: ForgeFilterContext): ForgeFilterContext {
	if (["user", "author", "assignee", "assignees", "merged_by", "closed_by"].includes(key)) return "user";
	if (key === "labels") return "label";
	if (["files", "diffs"].includes(key)) return "file";
	if (["commits", "commit"].includes(key)) return "commit";
	if (["check_runs", "statuses", "pipelines"].includes(key)) return "check";
	return current;
}

export function deleteForgeReplyFields(value: unknown, forge: ForgeName, context: ForgeFilterContext): unknown {
	if (Array.isArray(value)) return value.map((item) => deleteForgeReplyFields(item, forge, context));
	if (typeof value !== "object" || value === null) return value;
	const denied = new Set(FORGE_REPLY_DELETE_FIELDS[forge][context] ?? []);
	const filtered: Record<string, unknown> = {};
	for (const [key, item] of Object.entries(value)) {
		if (denied.has(key)) continue;
		filtered[key] = deleteForgeReplyFields(item, forge, nestedContext(key, context));
	}
	return filtered;
}

function includeGitlabNote(value: unknown): boolean {
	if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
	const note = value as Record<string, unknown>;
	return note.system === false && note.position === null && (note.type === null || note.type === undefined);
}

export function filterForgeReply(
	stdout: string,
	forge: ForgeName,
	context: ForgeFilterContext,
	ndjson = false,
): string {
	if (!ndjson) {
		try {
			return JSON.stringify(deleteForgeReplyFields(JSON.parse(stdout), forge, context));
		} catch {
			throw new Error("Native JSON reply was malformed");
		}
	}
	return stdout
		.split(/\r?\n/)
		.filter((line) => line !== "")
		.map((line, index) => {
			try {
				return JSON.parse(line) as unknown;
			} catch {
				throw new Error(`Native NDJSON reply line ${index + 1} was malformed`);
			}
		})
		.filter((value) => forge !== "gitlab" || context !== "comments" || includeGitlabNote(value))
		.map((value) => JSON.stringify(deleteForgeReplyFields(value, forge, context)))
		.join("\n");
}
