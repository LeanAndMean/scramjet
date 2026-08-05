export type ForgeName = "github" | "gitlab";
export type ForgeArtifactKind = "issue" | "pr";
export type ForgePrSection = "files" | "commits" | "checks";

export interface ForgeRepository {
	forge: ForgeName;
	host: "github.com" | "gitlab.com";
	projectPath: string;
}

export interface ForgeActor {
	login: string | null;
	kind: "user" | "bot" | "deleted";
}

export interface ForgeComment {
	id: string;
	url: string;
	author: ForgeActor;
	body: string;
	createdAt: string;
	updatedAt: string;
}

interface ForgeArtifactBase {
	kind: ForgeArtifactKind;
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

export interface ForgeIssueRelationship {
	relation: "parent" | "child";
	source: "native" | "task-list";
	number: number;
	url: string;
	state: string;
	title: string;
}

export interface ForgeIssueRelationships {
	capability: "supported" | "unsupported";
	items: ForgeIssueRelationship[];
}

export interface ForgeIssue extends ForgeArtifactBase {
	kind: "issue";
	relationships: ForgeIssueRelationships;
}

export interface ForgePrReadiness {
	draft: boolean;
	mergeable: "mergeable" | "conflicting" | "unknown";
	reviewDecision: string | null;
	head: string;
	base: string;
}

export interface ForgePrFile {
	path: string;
	status: string;
	additions: number;
	deletions: number;
	previousPath: string | null;
}

export interface ForgePrCommit {
	sha: string;
	title: string;
	author: string | null;
	createdAt: string;
	url: string | null;
}

export interface ForgePrCheck {
	id: string;
	name: string;
	status: string;
	conclusion: string | null;
	url: string | null;
}

export interface ForgePullRequest extends ForgeArtifactBase {
	kind: "pr";
	readiness: ForgePrReadiness;
	sections: {
		files?: ForgePrFile[];
		commits?: ForgePrCommit[];
		checks?: ForgePrCheck[];
	};
}

export type ForgeArtifact = ForgeIssue | ForgePullRequest;

export type ForgeMutationTarget = { kind: "artifact" } | { kind: "comment"; id: string };

export interface ForgeTextEdit {
	oldText: string;
	newText: string;
}

export interface ForgeArtifactIdentity {
	kind: ForgeArtifactKind;
	number: number;
	url: string;
}

export interface ForgeCommentIdentity {
	kind: "comment";
	id: string;
	url: string;
}

export type ForgeIdentity = ForgeArtifactIdentity | ForgeCommentIdentity;

export type ForgeCreateInput =
	| { kind: "issue"; title: string; body: string }
	| { kind: "pr"; title: string; body: string; head: string; base: string; draft: boolean };

export interface ForgeUpdateArtifactInput {
	kind: ForgeArtifactKind;
	number: number;
	title?: string;
	body?: string;
}

export interface ForgeAddCommentInput {
	kind: ForgeArtifactKind;
	number: number;
	body: string;
}

export interface ForgeUpdateCommentInput {
	kind: ForgeArtifactKind;
	number: number;
	id: string;
	body: string;
}

export interface ForgeAdapter {
	readArtifact(
		repository: ForgeRepository,
		kind: ForgeArtifactKind,
		number: number,
		include: readonly ForgePrSection[],
		signal?: AbortSignal,
	): Promise<ForgeArtifact>;
	createArtifact(
		repository: ForgeRepository,
		input: ForgeCreateInput,
		signal?: AbortSignal,
	): Promise<ForgeArtifactIdentity>;
	updateArtifact(
		repository: ForgeRepository,
		input: ForgeUpdateArtifactInput,
		signal?: AbortSignal,
	): Promise<ForgeArtifactIdentity>;
	addComment(
		repository: ForgeRepository,
		input: ForgeAddCommentInput,
		signal?: AbortSignal,
	): Promise<ForgeCommentIdentity>;
	updateComment(
		repository: ForgeRepository,
		input: ForgeUpdateCommentInput,
		signal?: AbortSignal,
	): Promise<ForgeCommentIdentity>;
}

export interface ForgeCoverageRange {
	start: number;
	end: number;
}

interface ForgeFieldCoverageBase {
	totalCodeUnits: number;
	ranges: ForgeCoverageRange[];
}

export type ForgeFieldCoverage =
	| (ForgeFieldCoverageBase & { target: { kind: "artifact" }; field: "title" | "body" })
	| (ForgeFieldCoverageBase & { target: { kind: "comment"; id: string }; field: "body" });

export interface ForgeReadDetails {
	schema: "scramjet:forge-read@1";
	repository: ForgeRepository;
	artifact: { kind: ForgeArtifactKind; number: number };
	snapshot: string;
	include: ForgePrSection[];
	range: { offset: number; lines: number; totalLines: number };
	fields: ForgeFieldCoverage[];
	core: { totalLines: number; ranges: ForgeCoverageRange[] };
}
