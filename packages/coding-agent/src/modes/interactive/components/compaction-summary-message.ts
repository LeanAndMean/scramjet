import {
	Box,
	getRenderedCopy,
	Markdown,
	type MarkdownTheme,
	Spacer,
	sanitizeUntrustedText,
	setRenderedCopy,
	Text,
} from "@leanandmean/tui";
import type { CompactionSummaryMessage } from "../../../core/messages.js";
import { getMarkdownTheme, theme } from "../theme/theme.js";
import { keyText } from "./keybinding-hints.js";

/**
 * Component that renders a compaction message with collapsed/expanded state.
 * Uses same background color as custom messages for visual consistency.
 */
export class CompactionSummaryMessageComponent extends Box {
	private expanded = false;
	private message: CompactionSummaryMessage;
	private markdownTheme: MarkdownTheme;

	constructor(message: CompactionSummaryMessage, markdownTheme: MarkdownTheme = getMarkdownTheme()) {
		super(1, 1, (t) => theme.bg("customMessageBg", t));
		this.message = message;
		this.markdownTheme = markdownTheme;
		this.updateDisplay();
	}

	setExpanded(expanded: boolean): void {
		this.expanded = expanded;
		this.updateDisplay();
	}

	override invalidate(): void {
		super.invalidate();
		this.updateDisplay();
	}

	private updateDisplay(): void {
		this.clear();

		const tokenStr = this.message.tokensBefore.toLocaleString();
		const label = theme.fg("customMessageLabel", `\x1b[1m[compaction]\x1b[22m`);
		this.addChild(new Text(label, 0, 0));
		this.addChild(new Spacer(1));

		// SCRAMJET-DIVERGENCE: qualify historical estimates and preview bounded, control-safe generated content.
		const count = `Estimated pre-compaction context: ~${tokenStr} tokens`;
		if (this.expanded) {
			const header =
				`**${count}**\n\n` +
				"Estimate uses available provider usage plus later-message estimates, otherwise conversation messages only; " +
				"not the summarization request size.\n\n";
			this.addChild(
				new Markdown(header + this.message.summary, 0, 0, this.markdownTheme, {
					color: (text: string) => theme.fg("customMessageText", text),
				}),
			);
		} else {
			this.addChild(new Text(theme.fg("customMessageText", count), 0, 0));
			this.addChild(new Spacer(1));
			let end = Math.min(4096, this.message.summary.length);
			if (end < this.message.summary.length && /[\uD800-\uDBFF]/.test(this.message.summary[end - 1])) end--;
			const sourceClipped = end < this.message.summary.length;
			const preview = new Markdown(
				sanitizeUntrustedText(this.message.summary.slice(0, end)),
				0,
				0,
				this.markdownTheme,
				{
					color: (text: string) => theme.fg("customMessageText", text),
				},
			);
			this.addChild({
				invalidate: () => preview.invalidate(),
				render: (width) => {
					const lines = preview.render(width);
					const result = lines.slice(0, 10);
					const copy = getRenderedCopy(lines).slice(0, 10);
					if (sourceClipped || lines.length > 10) {
						const omission = new Text(theme.fg("dim", "…"), 0, 0).render(width);
						result.push(...omission);
						copy.push(...getRenderedCopy(omission));
					}
					return setRenderedCopy(result, copy);
				},
			});
		}
		this.addChild(new Spacer(1));
		this.addChild(
			new Text(
				theme.fg("dim", `(${keyText("app.tools.expand")} to ${this.expanded ? "collapse" : "expand"})`),
				0,
				0,
			),
		);
	}
}
