// SCRAMJET-DIVERGENCE: scramjet-command block rendering (issue 82)

import { Box, Markdown, type MarkdownTheme, Text } from "@leanandmean/tui";
import type { ParsedScramjetCommandBlock } from "../../../core/scramjet-command-parser.js";
import { getMarkdownTheme, theme } from "../theme/theme.js";
import { keyText } from "./keybinding-hints.js";

export class ScramjetCommandMessageComponent extends Box {
	private expanded = false;
	private block: ParsedScramjetCommandBlock;
	private markdownTheme: MarkdownTheme;

	constructor(block: ParsedScramjetCommandBlock, markdownTheme: MarkdownTheme = getMarkdownTheme()) {
		super(1, 1, (t) => theme.bg("customMessageBg", t));
		this.block = block;
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
		if (this.expanded) {
			const label = theme.fg("customMessageLabel", `\x1b[1m[command]\x1b[22m`);
			this.addChild(new Text(label, 0, 0));
			const header = `**${this.block.name}**\n\n`;
			this.addChild(
				new Markdown(header + this.block.content, 0, 0, this.markdownTheme, {
					color: (text: string) => theme.fg("customMessageText", text),
				}),
			);
		} else {
			const label = `/${this.block.name}`;
			const args = this.block.userContext ? ` ${truncateArgs(this.block.userContext, 60)}` : "";
			const line =
				theme.fg("customMessageLabel", `\x1b[1m[command]\x1b[22m `) +
				theme.fg("customMessageText", label + args) +
				theme.fg("dim", ` (${keyText("app.tools.expand")} to expand)`);
			this.addChild(new Text(line, 0, 0));
		}
	}
}

function truncateArgs(args: string, maxLen: number): string {
	const singleLine = args.replace(/\n/g, " ");
	if (singleLine.length <= maxLen) return singleLine;
	return `${singleLine.slice(0, maxLen)}…`;
}
