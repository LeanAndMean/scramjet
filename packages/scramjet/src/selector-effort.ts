import type { ThinkingLevel } from "@leanandmean/agent";
import { getSupportedThinkingLevels, type Model } from "@leanandmean/ai";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { type Keybinding, type KeybindingsManager, truncateToWidth } from "@leanandmean/tui";

export interface SelectorEffortOptions {
	model: Model<any> | undefined;
	thinking: Pick<ExtensionAPI, "getThinkingLevel" | "setThinkingLevel">;
	keybindings: KeybindingsManager;
	protectedActions: readonly Keybinding[];
}

export interface SelectorEffortControl {
	handleInput(data: string): boolean;
	render(width: number, style?: (text: string) => string): string;
}

function keyIdentity(key: string): string {
	const parts = key.toLowerCase().split("+");
	const base = parts.at(-1);
	if (!base) return key.toLowerCase();
	const normalizedBase = base === "esc" ? "escape" : base === "return" ? "enter" : base;
	const modifiers = ["shift", "ctrl", "alt", "super"].filter((modifier) => parts.includes(modifier));
	return [...modifiers, normalizedBase].join("+");
}

function keysOverlap(left: string, right: string): boolean {
	const identities = new Set([keyIdentity(left), keyIdentity(right)]);
	if (identities.size === 1) return true;
	if (identities.has("escape") && identities.has("ctrl+[")) return true;
	return identities.has("enter") && (identities.has("ctrl+m") || identities.has("ctrl+j"));
}

export function createSelectorEffortControl({
	model,
	thinking,
	keybindings,
	protectedActions,
}: SelectorEffortOptions): SelectorEffortControl {
	const protectedKeys = protectedActions.flatMap((action) => keybindings.getKeys(action));
	const usableKeys = keybindings
		.getKeys("app.thinking.cycle")
		.filter((key) => !protectedKeys.some((protectedKey) => keysOverlap(key, protectedKey)));

	return {
		handleInput(data: string): boolean {
			if (protectedActions.some((action) => keybindings.matches(data, action))) return false;
			if (!keybindings.matches(data, "app.thinking.cycle")) return false;
			if (!model) return true;

			const levels = getSupportedThinkingLevels(model);
			if (levels.length < 2) return true;

			const current = thinking.getThinkingLevel();
			const currentIndex = levels.indexOf(current);
			const next = currentIndex === -1 ? levels[0] : levels[(currentIndex + 1) % levels.length];
			thinking.setThinkingLevel(next as ThinkingLevel);
			return true;
		},
		render(width: number, style = (text) => text): string {
			const shortcut = usableKeys.length > 0 ? ` • ${usableKeys.join("/")} cycle` : "";
			return truncateToWidth(style(`effort: ${thinking.getThinkingLevel()}${shortcut}`), width, "");
		},
	};
}
