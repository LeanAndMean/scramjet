/**
 * /clear — alias for /new, for muscle memory from Claude Code CLI.
 *
 * Pi calls it /new; Claude Code calls it /clear. Both audiences land
 * in the same place: a fresh session.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export function registerClearAlias(pi: ExtensionAPI) {
	pi.registerCommand("clear", {
		description: "Start a new session (alias for /new)",
		handler: async (_args, ctx) => {
			await ctx.newSession();
		},
	});
}
