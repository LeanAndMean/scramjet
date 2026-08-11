import { type ExtensionAPI, getMarkdownTheme } from "@leanandmean/coding-agent";
import { Markdown, Text, truncateToWidth } from "@leanandmean/tui";
import { Type } from "typebox";
import {
	type ForgeRepository,
	type IssueProposal,
	publishGithubIssue,
	resolveForgeOrigin,
	sameRepository,
} from "./forge-publication-provider.js";
import type { ScramjetState } from "./types.js";

const DETAILS_KIND = "scramjet:forge-publication";
const DISPLAY_MARKER = "⟦";
const VIEWPORT_LINES = 18;

interface PublicationDetails {
	kind: typeof DETAILS_KIND;
	operation: "create_issue";
	repository?: string;
	outcome: "cancelled" | "verified" | "headless" | "stale" | "pre-dispatch-failure" | "ambiguous";
	writeState: "not-dispatched" | "possible" | "verified";
	url?: string;
	retryProhibited?: true;
	reason?: string;
}

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
		if (event.toolName !== "create_issue" || !isPublicationDetails(event.details)) return;
		return { isError: !["cancelled", "verified"].includes(event.details.outcome) };
	});

	pi.registerTool({
		name: "create_issue",
		label: "Create Issue",
		description:
			"Create an issue in the current public GitHub repository after showing the exact title and body for interactive approval.",
		promptSnippet: "Create a GitHub issue with mandatory inline approval of the exact final title and body",
		executionMode: "sequential",
		parameters: Type.Object({
			title: Type.String({ minLength: 1, description: "Exact final issue title." }),
			body: Type.String({ description: "Exact final issue body." }),
		}),
		renderCall(args, theme) {
			return new Text(`${theme.fg("toolTitle", theme.bold("create_issue"))} ${theme.fg("muted", args.title)}`, 0, 0);
		},
		renderResult(result, _options, theme) {
			const details = result.details as PublicationDetails | undefined;
			if (!isPublicationDetails(details))
				return new Text(result.content.map((part) => (part.type === "text" ? part.text : "")).join("\n"), 0, 0);
			const color = details.outcome === "verified" ? "success" : details.outcome === "cancelled" ? "muted" : "error";
			return new Text(theme.fg(color, resultText(details)), 0, 0);
		},
		async execute(_toolCallId, params, signal, _onUpdate, ctx) {
			const proposal = freezeProposal(params.title, params.body);
			if (!isValidUnicode(proposal.title) || !isValidUnicode(proposal.body)) {
				return toolResult({
					outcome: "pre-dispatch-failure",
					writeState: "not-dispatched",
					reason: "proposal-is-not-valid-utf8",
				});
			}
			if (!ctx.hasUI || !ctx.ui) {
				return toolResult({
					outcome: "headless",
					writeState: "not-dispatched",
					reason: "interactive-approval-unavailable",
				});
			}

			let repository: ForgeRepository;
			try {
				repository = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd);
			} catch (error) {
				return toolResult({
					outcome: "pre-dispatch-failure",
					writeState: "not-dispatched",
					reason: safeError(error),
				});
			}
			if (repository.provider !== "github") {
				return toolResult({
					outcome: "pre-dispatch-failure",
					writeState: "not-dispatched",
					reason: "create_issue currently supports GitHub only",
				});
			}

			const expectedEpoch = sessionEpoch;
			const expectedGeneration = state.lifecycleGeneration;
			let approval: ApprovalResult;
			try {
				approval = await ctx.ui.custom<ApprovalResult>((tui, theme, keybindings, done) => {
					let finished = false;
					const finish = (result: ApprovalResult) => {
						if (finished) return;
						finished = true;
						activeApproval = undefined;
						done(result);
					};
					activeApproval = finish;
					return new ApprovalComponent(repository, proposal, tui, theme, keybindings, finish);
				});
			} catch {
				activeApproval = undefined;
				return toolResult(
					{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: "approval-ui-failed" },
					repository,
				);
			}
			if (approval !== "approved") {
				return toolResult(
					{ outcome: approval === "cancelled" ? "cancelled" : "stale", writeState: "not-dispatched" },
					repository,
				);
			}
			const isFresh = () =>
				runtimeLive &&
				sessionEpoch === expectedEpoch &&
				state.lifecycleGeneration === expectedGeneration &&
				!signal?.aborted;
			if (!isFresh()) return toolResult({ outcome: "stale", writeState: "not-dispatched" }, repository);

			let current: ForgeRepository;
			try {
				current = await resolveForgeOrigin(pi.exec.bind(pi), ctx.cwd);
			} catch {
				return toolResult(
					{ outcome: "stale", writeState: "not-dispatched", reason: "origin-could-not-be-revalidated" },
					repository,
				);
			}
			if (!isFresh()) return toolResult({ outcome: "stale", writeState: "not-dispatched" }, repository);
			if (!sameRepository(repository, current))
				return toolResult({ outcome: "stale", writeState: "not-dispatched", reason: "origin-changed" }, repository);

			const outcome = await publishGithubIssue(pi.exec.bind(pi), repository, proposal, ctx.cwd, signal);
			if (outcome.status === "verified")
				return toolResult({ outcome: "verified", writeState: "verified", url: outcome.url }, repository);
			if (outcome.status === "no-write")
				return toolResult(
					{ outcome: "pre-dispatch-failure", writeState: "not-dispatched", reason: outcome.reason },
					repository,
				);
			return toolResult(
				{ outcome: "ambiguous", writeState: "possible", reason: outcome.reason, retryProhibited: true },
				repository,
			);
		},
	});
}

type ApprovalResult = "approved" | "cancelled" | "stale";

class ApprovalComponent {
	private selected: "cancel" | "approve" = "cancel";
	private offset = 0;
	private contentLines: string[] = [];
	private width = 0;

	constructor(
		private readonly repository: Extract<ForgeRepository, { provider: "github" }>,
		private readonly proposal: IssueProposal,
		private readonly tui: { requestRender(): void },
		private readonly theme: any,
		private readonly keybindings: { matches(data: string, action: string): boolean },
		private readonly done: (result: ApprovalResult) => void,
	) {}

	render(width: number): string[] {
		if (width !== this.width) this.buildContent(width);
		const maxOffset = Math.max(0, this.contentLines.length - VIEWPORT_LINES);
		this.offset = Math.min(this.offset, maxOffset);
		const visible = this.contentLines.slice(this.offset, this.offset + VIEWPORT_LINES);
		const above = this.offset > 0 ? `${this.offset} lines above` : "Beginning of payload";
		const belowCount = Math.max(0, this.contentLines.length - this.offset - visible.length);
		const below = belowCount > 0 ? `${belowCount} lines below` : "End of payload";
		const cancel = this.selected === "cancel" ? this.theme.fg("accent", "[ Cancel ]") : "  Cancel  ";
		const approve =
			this.selected === "approve" ? this.theme.fg("warning", "[ Approve publication ]") : "  Approve publication  ";
		return [
			truncateToWidth(this.theme.fg("accent", this.theme.bold("Create GitHub issue — approval required")), width),
			truncateToWidth(`Repository: ${this.repository.owner}/${this.repository.repository}`, width),
			truncateToWidth("Consequence: publish a new issue visible to repository readers", width),
			truncateToWidth(this.theme.fg("dim", above), width),
			...visible.map((line) => truncateToWidth(line, width)),
			truncateToWidth(this.theme.fg("dim", below), width),
			truncateToWidth(`${cancel}   ${approve}`, width),
			truncateToWidth(
				this.theme.fg("dim", "↑↓/PgUp/PgDn scroll • Tab/←→ choose • Enter confirm • Esc cancel"),
				width,
			),
		];
	}

	handleInput(data: string): void {
		if (this.keybindings.matches(data, "tui.select.cancel")) {
			this.done("cancelled");
			return;
		}
		if (this.keybindings.matches(data, "tui.select.confirm")) {
			this.done(this.selected === "approve" ? "approved" : "cancelled");
			return;
		}
		if (data === "\t" || data === "\u001b[D" || data === "\u001b[C")
			this.selected = this.selected === "cancel" ? "approve" : "cancel";
		else if (this.keybindings.matches(data, "tui.select.up")) this.offset--;
		else if (this.keybindings.matches(data, "tui.select.down")) this.offset++;
		else if (data === "\u001b[5~") this.offset -= VIEWPORT_LINES;
		else if (data === "\u001b[6~") this.offset += VIEWPORT_LINES;
		this.offset = Math.max(0, Math.min(this.offset, Math.max(0, this.contentLines.length - VIEWPORT_LINES)));
		this.tui.requestRender();
	}

	invalidate(): void {
		this.width = 0;
	}

	dispose(): void {}

	private buildContent(width: number): void {
		this.width = width;
		const title = projectTerminalSafe(this.proposal.title);
		const body = projectTerminalSafe(this.proposal.body);
		const markdown = new Markdown(`## Title\n\n${title.text}\n\n## Body\n\n${body.text}`, 0, 0, getMarkdownTheme());
		this.contentLines = markdown.render(Math.max(1, width));
		if (title.changed || body.changed)
			this.contentLines.push(
				this.theme.fg(
					"warning",
					`${DISPLAY_MARKER}…${DISPLAY_MARKER} marks escaped terminal-unsafe or hyperlink syntax.`,
				),
			);
	}
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

function freezeProposal(title: string, body: string): IssueProposal {
	return Object.freeze({ title: `${title}`, body: `${body}` });
}

function toolResult(
	fields: Omit<PublicationDetails, "kind" | "operation" | "repository">,
	repository?: ForgeRepository,
) {
	const details: PublicationDetails = {
		kind: DETAILS_KIND,
		operation: "create_issue",
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
	if (details.outcome === "verified") return `Issue publication verified: ${details.url}`;
	if (details.outcome === "cancelled") return "Issue publication cancelled. No remote write was performed.";
	if (details.outcome === "ambiguous")
		return `Issue publication may have occurred; automatic retry is prohibited. Reconcile deliberately. (${details.reason})`;
	return `Issue publication did not begin: ${details.reason ?? details.outcome}.`;
}

function isPublicationDetails(value: unknown): value is PublicationDetails {
	if (!value || typeof value !== "object") return false;
	const details = value as Record<string, unknown>;
	return (
		details.kind === DETAILS_KIND &&
		details.operation === "create_issue" &&
		typeof details.outcome === "string" &&
		typeof details.writeState === "string"
	);
}

function safeError(error: unknown): string {
	return error instanceof Error ? error.message : "provider preflight failed";
}
