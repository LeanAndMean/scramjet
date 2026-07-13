import type { ExtensionAPI, ExtensionContext } from "@leanandmean/coding-agent";
import { showSettingsPage } from "./settings-ui.js";
import type { ScramjetState } from "./types.js";

export function registerScramjetCommand(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerCommand("scramjet", {
		description: "Scramjet harness commands: /scramjet settings",
		getArgumentCompletions: (prefix) => {
			const options = ["settings"];
			const filtered = options.filter((o) => o.startsWith(prefix));
			return filtered.length > 0 ? filtered.map((o) => ({ value: o, label: o })) : null;
		},
		handler: async (args, ctx) => {
			const arg = args.trim().toLowerCase();

			if (arg === "" || arg === "settings") {
				if (!ctx.hasUI) {
					ctx.ui.notify("Settings requires a TUI environment", "error");
					return;
				}
				await showSettingsPage(pi, ctx as ExtensionContext, state);
			} else {
				ctx.ui.notify("Usage: /scramjet settings", "warning");
			}
		},
	});
}
