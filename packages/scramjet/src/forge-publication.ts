import { type ExtensionAPI, getMarkdownTheme, getSelectListTheme } from "@leanandmean/coding-agent";
import {
	Box,
	type Component,
	Container,
	Markdown,
	SelectList,
	Spacer,
	sanitizeUntrustedText,
	Text,
	truncateToWidth,
} from "@leanandmean/tui";
import { Type } from "typebox";
import { loadAutonomyConfig, mergeAllRecommendations, resolvePublicationPolicy } from "./autonomy-settings.js";
import {
	type ForgeRepository,
	type PublicationRequest,
	preflightForgePublication,
	preflightPullRequestBranches,
	publishForge,
	resolveForgeOrigin,
	sameRepository,
} from "./forge-publication-provider.js";
import type { PublicationTool, ScramjetState } from "./types.js";
import { PUBLICATION_TOOLS } from "./types.js";

const DETAILS_KIND = "scramjet:forge-publication";
const OPERATIONS = PUBLICATION_TOOLS;
type Operation = PublicationTool;
type PublicationOutcomeDetails =
	| { outcome: "cancelled"; writeState: "not-dispatched" }
	| { outcome: "verified"; writeState: "verified"; url: string }
	| { outcome: "headless" | "stale" | "pre-dispatch-failure"; writeState: "not-dispatched"; reason?: string }
	| { outcome: "ambiguous"; writeState: "possible"; retryProhibited: true; reason?: string };
type PublicationAuthorization = { mode: "interactive" | "command-default" | "user-override"; command: string | null };
type PublicationDetails = {
	kind: typeof DETAILS_KIND;
	operation: Operation;
	repository?: string;
	authorization?: PublicationAuthorization;
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
			description: "Create an issue in the current public GitHub or GitLab repository under publication policy.",
			parameters: Type.Object({ title: Type.String({ minLength: 1 }), body: Type.String() }),
		},
		{
			name: "create_pr",
			label: "Create Pull Request",
			description:
				"Create a pull request or merge request from current-repository branches under publication policy.",
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
				"Add a comment to an issue in the current public GitHub or GitLab repository under publication policy.",
			parameters: Type.Object({ number: Type.Integer({ minimum: 1 }), body: Type.String() }),
		},
		{
			name: "add_pr_comment",
			label: "Add PR Comment",
			description:
				"Add a comment to a pull request or merge request in the current public GitHub or GitLab repository under publication policy.",
			parameters: Type.Object({ number: Type.Integer({ minimum: 1 }), body: Type.String() }),
		},
	] as const;
	for (const definition of definitions as readonly any[]) {
		pi.registerTool({
			...definition,
			promptSnippet: `${definition.name}: explain the decision context and consequences concisely, then supply the complete final content only in this tool call. Do not repeat the full payload in prose. Publication follows user policy and always requires exact verification; never retry an ambiguous result automatically.`,
			executionMode: "sequential",
			renderCall(args: any, theme, { expanded }) {
				const request = freezeRequest(definition.name, args);
				if (expanded) return publicationPayloadComponent(request, theme);
				return new Text(
					`${theme.fg("toolTitle", theme.bold(definition.name))} ${theme.fg("muted", sanitizeUntrustedText(callSummary(request)))}`,
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
						sanitizeUntrustedText(resultText(details)),
					),
					0,
					0,
				);
			},
			async execute(toolCallId, params, signal, _update, ctx) {
				const request = freezeRequest(definition.name, params);
				const expectedEpoch = sessionEpoch;
				const expectedGeneration = state.lifecycleGeneration;
				const expectedCommand = state.lifecycle.activeCommand;
				const resolveDecision = () => {
					const command = expectedCommand ? state.registry.get(expectedCommand) : undefined;
					if (!command || command.delegateOnly || !command.allowedTools?.includes(request.operation)) {
						return { policy: "require-approval", authorization: "interactive" } as const;
					}
					try {
						return resolvePublicationPolicy(
							loadAutonomyConfig(state.autonomyConfigPath, true),
							mergeAllRecommendations(state.autonomyRecommendations),
							expectedCommand,
							request.operation,
						);
					} catch (error) {
						state.logger.warn("scope", `publication autonomy ignored: ${safeError(error)}`);
						return { policy: "require-approval", authorization: "interactive" } as const;
					}
				};
				const decision = resolveDecision();
				const policy = decision.policy;
				if (!requestStrings(request).every(isValidUnicode))
					return toolResult(request.operation, {
						outcome: "pre-dispatch-failure",
						writeState: "not-dispatched",
						reason: "proposal-is-not-valid-utf8",
					});
				if (
					(request.operation === "create_issue" || request.operation === "create_pr") &&
					/[\r\n]/.test(request.title)
				)
					return toolResult(request.operation, {
						outcome: "pre-dispatch-failure",
						writeState: "not-dispatched",
						reason: "publication titles must contain exactly one line",
					});
				if (policy === "require-approval" && (!ctx.hasUI || !ctx.ui))
					return toolResult(request.operation, {
						outcome: "headless",
						writeState: "not-dispatched",
						reason: "interactive-approval-unavailable",
					});
				let repository: ForgeRepository;
				try {
					repository = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd, signal);
				} catch (error) {
					return toolResult(request.operation, {
						outcome: "pre-dispatch-failure",
						writeState: "not-dispatched",
						reason: safeError(error),
					});
				}
				if (
					request.operation === "create_pr" &&
					repository.provider === "gitlab" &&
					/^(?:Draft:|WIP:)/i.test(request.title) !== request.draft
				)
					return toolResult(
						request.operation,
						{
							outcome: "pre-dispatch-failure",
							writeState: "not-dispatched",
							reason: "GitLab draft state must match an approved Draft: or WIP: title prefix",
						},
						repository,
					);
				try {
					await preflightForgePublication(pi.exec.bind(pi), repository, request, ctx.cwd, signal);
					if (request.operation === "create_pr")
						await preflightPullRequestBranches(pi.exec.bind(pi), request, ctx.cwd, signal);
				} catch (error) {
					return toolResult(
						request.operation,
						{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: safeError(error) },
						repository,
					);
				}
				const authorization: PublicationAuthorization = {
					mode: decision.authorization,
					command: expectedCommand,
				};
				let approval: ApprovalResult = "approved";
				if (policy === "require-approval") {
					let removeApprovalAbort = () => {};
					try {
						approval = await ctx.ui!.custom<ApprovalResult>(
							(tui, theme, _keybindings, done) => {
								let finished = false;
								const finish = (result: ApprovalResult) => {
									if (finished) return;
									finished = true;
									removeApprovalAbort();
									if (activeApproval === finish) activeApproval = undefined;
									done(result);
								};
								const abortApproval = () => finish("stale");
								if (signal) {
									signal.addEventListener("abort", abortApproval, { once: true });
									removeApprovalAbort = () => signal.removeEventListener("abort", abortApproval);
								}
								activeApproval = finish;
								if (signal?.aborted) abortApproval();
								return new ApprovalComponent(tui, theme, repository, request, finish);
							},
							{
								toolAttachedContext: {
									toolCallId,
									render: (_tui, previewTheme) => approvalPreviewComponent(repository, request, previewTheme),
								},
							},
						);
					} catch (error) {
						removeApprovalAbort();
						activeApproval = undefined;
						const reason = sanitizeUntrustedText(safeError(error));
						state.logger.warn("scope", `publication approval UI failed: ${reason}`);
						return toolResult(
							request.operation,
							{
								outcome: "pre-dispatch-failure",
								writeState: "not-dispatched",
								reason: `approval-ui-failed: ${reason}`,
							},
							repository,
						);
					}
					removeApprovalAbort();
					if (approval === undefined) {
						activeApproval = undefined;
						return toolResult(
							request.operation,
							{
								outcome: "pre-dispatch-failure",
								writeState: "not-dispatched",
								reason: "interactive-approval-unavailable",
							},
							repository,
							authorization,
						);
					}
				}
				if (approval !== "approved")
					return toolResult(
						request.operation,
						{ outcome: approval === "cancelled" ? "cancelled" : "stale", writeState: "not-dispatched" },
						repository,
						authorization,
					);
				const fresh = () => {
					const currentDecision = resolveDecision();
					return (
						runtimeLive &&
						sessionEpoch === expectedEpoch &&
						state.lifecycleGeneration === expectedGeneration &&
						state.lifecycle.activeCommand === expectedCommand &&
						currentDecision.policy === decision.policy &&
						currentDecision.authorization === decision.authorization &&
						!signal?.aborted
					);
				};
				if (!fresh())
					return toolResult(
						request.operation,
						{ outcome: "stale", writeState: "not-dispatched" },
						repository,
						authorization,
					);
				let current: ForgeRepository;
				try {
					current = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd, signal);
				} catch {
					return toolResult(
						request.operation,
						{ outcome: "stale", writeState: "not-dispatched", reason: "origin-could-not-be-revalidated" },
						repository,
						authorization,
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
						authorization,
					);
				try {
					await preflightForgePublication(pi.exec.bind(pi), repository, request, ctx.cwd, signal);
					if (request.operation === "create_pr")
						await preflightPullRequestBranches(pi.exec.bind(pi), request, ctx.cwd, signal);
				} catch (error) {
					return toolResult(
						request.operation,
						{ outcome: "stale", writeState: "not-dispatched", reason: safeError(error) },
						repository,
						authorization,
					);
				}
				if (!fresh())
					return toolResult(
						request.operation,
						{ outcome: "stale", writeState: "not-dispatched" },
						repository,
						authorization,
					);
				const outcome = await publishForge(pi.exec.bind(pi), repository, request, ctx.cwd, signal);
				if (outcome.status === "verified")
					return toolResult(
						request.operation,
						{ outcome: "verified", writeState: "verified", url: outcome.url },
						repository,
						authorization,
					);
				if (outcome.status === "no-write")
					return toolResult(
						request.operation,
						{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: outcome.reason },
						repository,
						authorization,
					);
				return toolResult(
					request.operation,
					{ outcome: "ambiguous", writeState: "possible", reason: outcome.reason, retryProhibited: true },
					repository,
					authorization,
				);
			},
		});
	}
}

class ApprovalComponent {
	private readonly choices: SelectList;
	constructor(
		private readonly tui: { requestRender(): void },
		private readonly theme: any,
		private readonly repository: ForgeRepository,
		private readonly request: PublicationRequest,
		done: (result: ApprovalResult) => void,
	) {
		this.choices = new SelectList(
			[
				{ value: "approve", label: "Approve publication" },
				{ value: "cancel", label: "Cancel" },
			],
			2,
			getSelectListTheme(),
		);
		this.choices.onSelect = (choice) => done(choice.value === "approve" ? "approved" : "cancelled");
		this.choices.onCancel = () => done("cancelled");
	}
	render(width: number): string[] {
		const context = new Text(
			this.theme.fg(
				"muted",
				`${operationLabel(this.request.operation)} in ${sanitizeUntrustedText(repositoryName(this.repository) ?? "")} — ${consequence(this.request)}`,
			),
			0,
			1,
		);
		return [
			...context.render(width),
			...this.choices.render(width),
			truncateToWidth(this.theme.fg("dim", "Esc cancel • ↑↓ • Enter"), width),
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

function publicationPayloadComponent(request: PublicationRequest, _theme: any): Container {
	const component = new Container();
	component.addChild(
		new Markdown(
			displayFields(request)
				.map(([label, value]) => `## ${label}\n\n${value}`)
				.join("\n\n"),
			0,
			0,
			getMarkdownTheme(),
			undefined,
			{ contentMode: "untrusted" },
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
		component.addChild(new Spacer(1));
		const card = new Box(1, 1, (text: string) => this.theme.bg("toolPendingBg", text));
		card.addChild(
			new Text(
				[
					this.theme.fg("toolTitle", this.theme.bold(operationLabel(this.request.operation))),
					`Provider: ${this.repository.provider}`,
					`Repository: ${sanitizeUntrustedText(repositoryName(this.repository) ?? "")}`,
					`Consequence: ${consequence(this.request)}`,
				].join("\n"),
				0,
				0,
			),
		);
		card.addChild(publicationPayloadComponent(this.request, this.theme));
		component.addChild(card);
		return component.render(width);
	}
	invalidate(): void {}
}

function approvalPreviewComponent(repository: ForgeRepository, request: PublicationRequest, theme: any): Component {
	return new ApprovalPreviewComponent(repository, request, theme);
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
function toolResult(
	operation: Operation,
	fields: PublicationOutcomeDetails,
	repository?: ForgeRepository,
	authorization?: PublicationAuthorization,
) {
	const details: PublicationDetails = {
		kind: DETAILS_KIND,
		operation,
		repository: repositoryName(repository),
		authorization,
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
