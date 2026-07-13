/** /autopilot on|off — gates `closed`/`open`/`ask` decisions. `forced`
 *  fires regardless; see CLAUDE.md "MVP design rationales". */

import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { ENABLED_TOGGLE_TYPE, type EnabledToggleData } from "./history.js";
import type { ScramjetState } from "./types.js";

export function registerAutopilotCommand(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerCommand("autopilot", {
		description: "Toggle Scramjet auto-continuation: /autopilot on|off|status",
		getArgumentCompletions: (prefix) => {
			const options = ["on", "off", "status"];
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
			} else if (arg === "" || arg === "status") {
				ctx.ui.notify(`Autopilot is ${state.enabled ? "on" : "off"}`, "info");
			} else {
				ctx.ui.notify("Usage: /autopilot on|off|status", "warning");
			}
		},
	});
}
