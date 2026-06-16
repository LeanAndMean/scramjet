/** /scramjet on|off — gates `closed`/`open`/`ask` decisions. `forced`
 *  fires regardless; see CLAUDE.md "MVP design rationales". */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { ENABLED_TOGGLE_TYPE, type EnabledToggleData } from "./history.ts";
import { showSettingsPage } from "./settings-ui.ts";
import type { ScramjetState } from "./types.ts";

export function registerScramjetCommand(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerCommand("scramjet", {
		description: "Toggle Scramjet auto-continuation: /scramjet on|off|settings|status",
		getArgumentCompletions: (prefix) => {
			const options = ["on", "off", "settings", "status"];
			const filtered = options.filter((o) => o.startsWith(prefix));
			return filtered.length > 0 ? filtered.map((o) => ({ value: o, label: o })) : null;
		},
		handler: async (args, ctx) => {
			const arg = args.trim().toLowerCase();

			if (arg === "on") {
				state.enabled = true;
				const payload: EnabledToggleData = { enabled: true };
				pi.appendEntry(ENABLED_TOGGLE_TYPE, payload);
				ctx.ui.notify("Scramjet auto-continuation enabled", "info");
			} else if (arg === "off") {
				state.enabled = false;
				const payload: EnabledToggleData = { enabled: false };
				pi.appendEntry(ENABLED_TOGGLE_TYPE, payload);
				ctx.ui.notify("Scramjet auto-continuation disabled", "info");
			} else if (arg === "settings") {
				if (!ctx.hasUI) {
					ctx.ui.notify("Settings requires a TUI environment", "error");
					return;
				}
				await showSettingsPage(pi, ctx as ExtensionContext, state);
			} else if (arg === "" || arg === "status") {
				ctx.ui.notify(`Scramjet is ${state.enabled ? "on" : "off"}`, "info");
			} else {
				ctx.ui.notify("Usage: /scramjet on|off|settings|status", "warning");
			}
		},
	});
}
