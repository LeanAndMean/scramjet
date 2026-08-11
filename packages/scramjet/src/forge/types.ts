export type ForgeName = "github" | "gitlab";
export type ForgeArtifactKind = "issue" | "pr";

export interface ForgeRepository {
	forge: ForgeName;
	host: "github.com" | "gitlab.com";
	projectPath: string;
}

export type ForgeReadSegmentId =
	| "artifact"
	| "comments"
	| "sub_issues"
	| "parent"
	| "relationships"
	| "files"
	| "commits"
	| "check_runs"
	| "status"
	| "pipelines";

export type ForgeReplyShape =
	| { kind: "json" }
	| { kind: "gh-slurp"; itemsPath?: readonly string[] }
	| { kind: "ndjson" };

export interface ForgeReadPlanSegment {
	id: ForgeReadSegmentId;
	command: "gh" | "glab";
	args: string[];
	shape: ForgeReplyShape;
	optional?: boolean;
	evidence?: "artifact" | "comments";
}

export interface ForgeReadPlan {
	repository: ForgeRepository;
	artifact: { kind: ForgeArtifactKind; number: number };
	include: ForgeReadSegmentId[];
	segments: ForgeReadPlanSegment[];
}

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

export type ForgeEditable =
	| {
			target: { kind: "artifact" };
			kind: "issue";
			number: number;
			url: string;
			title: string;
			body: string;
	  }
	| {
			target: { kind: "artifact" };
			kind: "pr";
			number: number;
			url: string;
			title: string;
			body: string;
			draft: boolean;
			head: string;
			base: string;
	  }
	| {
			target: { kind: "comment"; id: string };
			kind: ForgeArtifactKind;
			number: number;
			url: string;
			body: string;
	  };

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
	readPlan(
		repository: ForgeRepository,
		kind: ForgeArtifactKind,
		number: number,
		include: readonly ForgeReadSegmentId[],
	): ForgeReadPlan;
	readEditable(
		repository: ForgeRepository,
		kind: ForgeArtifactKind,
		number: number,
		target: ForgeMutationTarget,
		signal?: AbortSignal,
	): Promise<ForgeEditable>;
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

export type ForgeSegmentCoverage =
	| { unit: "items"; offset: number; count: number; totalItems: number }
	| { unit: "bytes"; item: number; offset: number; bytes: number; totalBytes: number; totalItems: number };

export interface ForgePayloadRange {
	start: number;
	end: number;
}

export interface ForgeReadSegmentReceipt {
	id: ForgeReadSegmentId;
	status: "ok" | "optional_error";
	snapshot?: string;
	evidence?: "artifact" | "comments";
	coverage?: ForgeSegmentCoverage;
	payload: {
		segment: ForgePayloadRange;
		command?: ForgePayloadRange;
		output: ForgePayloadRange;
	};
}

export interface ForgeReadDetails {
	schema: "scramjet:forge-read@2";
	repository: ForgeRepository;
	artifact: { kind: ForgeArtifactKind; number: number };
	snapshot: string;
	include: ForgeReadSegmentId[];
	segments: ForgeReadSegmentReceipt[];
}
