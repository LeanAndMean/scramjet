import { type ExtensionAPI, getMarkdownTheme, getSelectListTheme } from "@leanandmean/coding-agent";
import { type Component, Container, Markdown, SelectList, Text, truncateToWidth } from "@leanandmean/tui";
import { Type } from "typebox";
import {
	type ForgeRepository,
	type PublicationRequest,
	preflightPullRequestBranches,
	publishForge,
	resolveForgeOrigin,
	sameRepository,
} from "./forge-publication-provider.js";
import type { ScramjetState } from "./types.js";

const DETAILS_KIND = "scramjet:forge-publication";
const DISPLAY_MARKER = "⟦";
const OPERATIONS = ["create_issue", "create_pr", "add_issue_comment", "add_pr_comment"] as const;
type Operation = (typeof OPERATIONS)[number];
type PublicationOutcomeDetails =
	| { outcome: "cancelled"; writeState: "not-dispatched" }
	| { outcome: "verified"; writeState: "verified"; url: string }
	| { outcome: "headless" | "stale" | "pre-dispatch-failure"; writeState: "not-dispatched"; reason?: string }
	| { outcome: "ambiguous"; writeState: "possible"; retryProhibited: true; reason?: string };
type PublicationDetails = {
	kind: typeof DETAILS_KIND;
	operation: Operation;
	repository?: string;
} & PublicationOutcomeDetails;
type ApprovalResult = "approved" | "cancelled" | "stale";

export function registerForgePublication(pi: ExtensionAPI, state: ScramjetState): void {
	let sessionEpoch = 0;
	let runtimeLive = true;
	let activeApproval: ((result: ApprovalResult) => void) | undefined;
	pi.on("session_shutdown", () => {
		runtimeLive = false;
		sessionEpoch++;
		activeApproval?.("stale");
		activeApproval = undefined;
	});
	pi.on("tool_result", (event) => {
		if (
			!OPERATIONS.includes(event.toolName as Operation) ||
			!isPublicationDetails(event.details) ||
			event.details.operation !== event.toolName
		)
			return;
		return { isError: !["cancelled", "verified"].includes(event.details.outcome) };
	});

	const definitions = [
		{
			name: "create_issue",
			label: "Create Issue",
			description: "Create an issue in the current public GitHub or GitLab repository after interactive approval.",
			parameters: Type.Object({ title: Type.String({ minLength: 1 }), body: Type.String() }),
		},
		{
			name: "create_pr",
			label: "Create Pull Request",
			description:
				"Create a pull request or merge request from current-repository branches after interactive approval.",
			parameters: Type.Object({
				title: Type.String({ minLength: 1 }),
				body: Type.String(),
				head: Type.String({ minLength: 1 }),
				base: Type.String({ minLength: 1 }),
				draft: Type.Boolean(),
			}),
		},
		{
			name: "add_issue_comment",
			label: "Add Issue Comment",
			description:
				"Add a comment to an issue in the current public GitHub or GitLab repository after interactive approval.",
			parameters: Type.Object({ number: Type.Integer({ minimum: 1 }), body: Type.String() }),
		},
		{
			name: "add_pr_comment",
			label: "Add PR Comment",
			description:
				"Add a comment to a pull request or merge request in the current public GitHub or GitLab repository after interactive approval.",
			parameters: Type.Object({ number: Type.Integer({ minimum: 1 }), body: Type.String() }),
		},
	] as const;
	for (const definition of definitions as readonly any[]) {
		pi.registerTool({
			...definition,
			promptSnippet: `${definition.name}: explain the decision context and consequences concisely, then supply the complete final content only in this tool call. Do not repeat the full payload in prose. Publication requires interactive approval and exact verification; never retry an ambiguous result automatically.`,
			executionMode: "sequential",
			renderCall(args: any, theme, { expanded }) {
				const request = freezeRequest(definition.name, args);
				if (expanded) return publicationPayloadComponent(request, theme);
				return new Text(
					`${theme.fg("toolTitle", theme.bold(definition.name))} ${theme.fg("muted", projectTerminalSafe(callSummary(request)).text)}`,
					0,
					0,
				);
			},
			renderResult(result, _options, theme) {
				const details = result.details as PublicationDetails | undefined;
				if (!isPublicationDetails(details))
					return new Text(result.content.map((part) => (part.type === "text" ? part.text : "")).join("\n"), 0, 0);
				return new Text(
					theme.fg(
						details.outcome === "verified" ? "success" : details.outcome === "cancelled" ? "muted" : "error",
						resultText(details),
					),
					0,
					0,
				);
			},
			async execute(_id, params, signal, _update, ctx) {
				const request = freezeRequest(definition.name, params);
				if (!requestStrings(request).every(isValidUnicode))
					return toolResult(request.operation, {
						outcome: "pre-dispatch-failure",
						writeState: "not-dispatched",
						reason: "proposal-is-not-valid-utf8",
					});
				if (!ctx.hasUI || !ctx.ui)
					return toolResult(request.operation, {
						outcome: "headless",
						writeState: "not-dispatched",
						reason: "interactive-approval-unavailable",
					});
				let repository: ForgeRepository;
				try {
					repository = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd);
				} catch (error) {
					return toolResult(request.operation, {
						outcome: "pre-dispatch-failure",
						writeState: "not-dispatched",
						reason: safeError(error),
					});
				}
				if (request.operation === "create_pr") {
					if (repository.provider === "gitlab" && request.draft && !/^(?:Draft:|WIP:)/i.test(request.title))
						return toolResult(
							request.operation,
							{
								outcome: "pre-dispatch-failure",
								writeState: "not-dispatched",
								reason: "GitLab draft titles must include an approved Draft: or WIP: prefix",
							},
							repository,
						);
					try {
						await preflightPullRequestBranches(pi.exec.bind(pi), request, ctx.cwd);
					} catch (error) {
						return toolResult(
							request.operation,
							{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: safeError(error) },
							repository,
						);
					}
				}
				const expectedEpoch = sessionEpoch;
				const expectedGeneration = state.lifecycleGeneration;
				let approval: ApprovalResult;
				try {
					approval = await ctx.ui.custom<ApprovalResult>(
						(tui, theme, _keybindings, done) => {
							let finished = false;
							const finish = (result: ApprovalResult) => {
								if (finished) return;
								finished = true;
								activeApproval = undefined;
								done(result);
							};
							activeApproval = finish;
							return new ApprovalComponent(repository, request, tui, theme, finish);
						},
						{
							committedPreview: (_tui, previewTheme) =>
								approvalPreviewComponent(repository, request, previewTheme),
						},
					);
				} catch {
					activeApproval = undefined;
					return toolResult(
						request.operation,
						{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: "approval-ui-failed" },
						repository,
					);
				}
				if (approval !== "approved")
					return toolResult(
						request.operation,
						{ outcome: approval === "cancelled" ? "cancelled" : "stale", writeState: "not-dispatched" },
						repository,
					);
				const fresh = () =>
					runtimeLive &&
					sessionEpoch === expectedEpoch &&
					state.lifecycleGeneration === expectedGeneration &&
					!signal?.aborted;
				if (!fresh())
					return toolResult(request.operation, { outcome: "stale", writeState: "not-dispatched" }, repository);
				let current: ForgeRepository;
				try {
					current = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd);
				} catch {
					return toolResult(
						request.operation,
						{ outcome: "stale", writeState: "not-dispatched", reason: "origin-could-not-be-revalidated" },
						repository,
					);
				}
				if (!fresh() || !sameRepository(repository, current))
					return toolResult(
						request.operation,
						{
							outcome: "stale",
							writeState: "not-dispatched",
							reason: sameRepository(repository, current) ? undefined : "origin-changed",
						},
						repository,
					);
				const outcome = await publishForge(pi.exec.bind(pi), repository, request, ctx.cwd, signal);
				if (outcome.status === "verified")
					return toolResult(
						request.operation,
						{ outcome: "verified", writeState: "verified", url: outcome.url },
						repository,
					);
				if (outcome.status === "no-write")
					return toolResult(
						request.operation,
						{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: outcome.reason },
						repository,
					);
				return toolResult(
					request.operation,
					{ outcome: "ambiguous", writeState: "possible", reason: outcome.reason, retryProhibited: true },
					repository,
				);
			},
		});
	}
}

class ApprovalComponent {
	private readonly choices: SelectList;
	constructor(
		private readonly repository: ForgeRepository,
		private readonly request: PublicationRequest,
		private readonly tui: { requestRender(): void },
		private readonly theme: any,
		done: (result: ApprovalResult) => void,
	) {
		this.choices = new SelectList(
			[
				{ value: "cancel", label: "Cancel" },
				{ value: "approve", label: "Approve publication" },
			],
			2,
			getSelectListTheme(),
		);
		this.choices.onSelect = (choice) => done(choice.value === "approve" ? "approved" : "cancelled");
		this.choices.onCancel = () => done("cancelled");
	}
	render(width: number): string[] {
		return [
			truncateToWidth(
				this.theme.fg("accent", this.theme.bold(`${operationLabel(this.request.operation)} — approval required`)),
				width,
			),
			truncateToWidth(`Provider: ${this.repository.provider}`, width),
			truncateToWidth(`Repository: ${repositoryName(this.repository)}`, width),
			truncateToWidth(`Consequence: ${consequence(this.request)}`, width),
			...this.choices.render(width),
			truncateToWidth(this.theme.fg("dim", "↑↓ choose • Enter confirm • Esc cancel"), width),
		];
	}
	handleInput(data: string): void {
		this.choices.handleInput(data);
		this.tui.requestRender();
	}
	invalidate(): void {
		this.choices.invalidate();
	}
	dispose(): void {}
}

function publicationPayloadComponent(request: PublicationRequest, theme: any): Container {
	const fields = displayFields(request).map(([label, value]) => ({ label, ...projectTerminalSafe(value) }));
	const component = new Container();
	component.addChild(
		new Markdown(fields.map((field) => `## ${field.label}\n\n${field.text}`).join("\n\n"), 0, 0, getMarkdownTheme()),
	);
	if (fields.some((field) => field.changed))
		component.addChild(
			new Text(
				theme.fg(
					"warning",
					`${DISPLAY_MARKER}…${DISPLAY_MARKER} marks escaped terminal-unsafe or hyperlink syntax.`,
				),
				0,
				0,
			),
		);
	return component;
}

class ApprovalPreviewComponent implements Component {
	constructor(
		private readonly repository: ForgeRepository,
		private readonly request: PublicationRequest,
		private readonly theme: any,
	) {}
	render(width: number): string[] {
		const component = new Container();
		component.addChild(
			new Text(
				[
					this.theme.fg(
						"accent",
						this.theme.bold(`${operationLabel(this.request.operation)} — approval required`),
					),
					`Provider: ${this.repository.provider}`,
					`Repository: ${repositoryName(this.repository)}`,
					`Consequence: ${consequence(this.request)}`,
				].join("\n"),
				0,
				0,
			),
		);
		component.addChild(publicationPayloadComponent(this.request, this.theme));
		return component.render(width);
	}
	invalidate(): void {}
}

function approvalPreviewComponent(repository: ForgeRepository, request: PublicationRequest, theme: any): Component {
	return new ApprovalPreviewComponent(repository, request, theme);
}

export function projectTerminalSafe(input: string): { text: string; changed: boolean; restore(): string } {
	let changed = false;
	let text = "";
	for (let index = 0; index < input.length; index++) {
		const code = input.charCodeAt(index);
		const point = input.codePointAt(index)!;
		if (point > 0xffff) index++;
		const prefix = input.slice(Math.max(0, index - 8), index).toLowerCase();
		const unsafe =
			code === 0x3c ||
			code === 0x5b ||
			code === 0x5d ||
			(code === 0x3a && /(?:https?|mailto)$/.test(prefix)) ||
			point === DISPLAY_MARKER.codePointAt(0) ||
			(point <= 0x1f && point !== 0x0a) ||
			(point >= 0x7f && point <= 0x9f) ||
			(point >= 0x202a && point <= 0x202e) ||
			(point >= 0x2066 && point <= 0x2069) ||
			(point & 0xffff) === 0xffff ||
			(point & 0xffff) === 0xfffe;
		if (unsafe) {
			changed = true;
			text += `${DISPLAY_MARKER}${point.toString(16).toUpperCase()}${DISPLAY_MARKER}`;
		} else text += String.fromCodePoint(point);
	}
	return {
		text,
		changed,
		restore: () =>
			text.replace(/⟦([0-9A-F]+)⟦/g, (_match, hex: string) => String.fromCodePoint(Number.parseInt(hex, 16))),
	};
}

function isValidUnicode(value: string): boolean {
	for (let index = 0; index < value.length; index++) {
		const code = value.charCodeAt(index);
		if (code >= 0xd800 && code <= 0xdbff) {
			const next = value.charCodeAt(++index);
			if (next < 0xdc00 || next > 0xdfff) return false;
		} else if (code >= 0xdc00 && code <= 0xdfff) return false;
	}
	return Buffer.from(value, "utf8").toString("utf8") === value;
}
function freezeRequest(operation: Operation, params: any): PublicationRequest {
	if (operation === "create_issue")
		return Object.freeze({ operation, title: `${params.title}`, body: `${params.body}` });
	if (operation === "create_pr")
		return Object.freeze({
			operation,
			title: `${params.title}`,
			body: `${params.body}`,
			head: `${params.head}`,
			base: `${params.base}`,
			draft: params.draft === true,
		});
	return Object.freeze({ operation, number: params.number, body: `${params.body}` });
}
function requestStrings(request: PublicationRequest): string[] {
	return request.operation === "create_pr"
		? [request.title, request.body, request.head, request.base]
		: request.operation === "create_issue"
			? [request.title, request.body]
			: [request.body];
}
function displayFields(request: PublicationRequest): [string, string][] {
	if (request.operation === "create_issue")
		return [
			["Title", request.title],
			["Body", request.body],
		];
	if (request.operation === "create_pr")
		return [
			["Title", request.title],
			["Head", request.head],
			["Base", request.base],
			["Draft", String(request.draft)],
			["Body", request.body],
		];
	return [
		["Target", `#${request.number}`],
		["Comment", request.body],
	];
}
function callSummary(request: PublicationRequest): string {
	if (request.operation === "create_issue") return request.title;
	if (request.operation === "create_pr")
		return `${request.title} (${request.head} → ${request.base}${request.draft ? ", draft" : ""})`;
	return `#${request.number}`;
}
function consequence(request: PublicationRequest): string {
	return request.operation === "create_issue"
		? "publish a new issue"
		: request.operation === "create_pr"
			? "publish a new pull/merge request"
			: `publish a comment on ${request.operation === "add_pr_comment" ? "pull/merge request" : "issue"} #${request.number}`;
}
function operationLabel(operation: Operation): string {
	return {
		create_issue: "Create issue",
		create_pr: "Create pull/merge request",
		add_issue_comment: "Add issue comment",
		add_pr_comment: "Add pull/merge request comment",
	}[operation];
}
function toolResult(operation: Operation, fields: PublicationOutcomeDetails, repository?: ForgeRepository) {
	const details: PublicationDetails = {
		kind: DETAILS_KIND,
		operation,
		repository: repositoryName(repository),
		...fields,
	};
	return { content: [{ type: "text" as const, text: resultText(details) }], details };
}
function repositoryName(repository?: ForgeRepository): string | undefined {
	if (!repository) return undefined;
	return repository.provider === "github"
		? `${repository.owner}/${repository.repository}`
		: `${repository.namespace}/${repository.repository}`;
}
function resultText(details: PublicationDetails): string {
	const noun = operationLabel(details.operation);
	if (details.outcome === "verified") return `${noun} publication verified: ${details.url}`;
	if (details.outcome === "cancelled") return `${noun} publication cancelled. No remote write was performed.`;
	if (details.outcome === "ambiguous")
		return `${noun} publication may have occurred; automatic retry is prohibited. Reconcile deliberately. (${details.reason})`;
	return `${noun} publication did not begin: ${details.reason ?? details.outcome}.`;
}
function isPublicationDetails(value: unknown): value is PublicationDetails {
	if (!value || typeof value !== "object") return false;
	const details = value as Record<string, unknown>;
	return (
		details.kind === DETAILS_KIND &&
		OPERATIONS.includes(details.operation as Operation) &&
		typeof details.outcome === "string" &&
		typeof details.writeState === "string"
	);
}
function safeError(error: unknown): string {
	return error instanceof Error ? error.message : "provider preflight failed";
}
