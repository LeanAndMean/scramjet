/** /clear — muscle-memory alias for Pi's /new. */

import type { ExtensionAPI } from "@leanandmean/coding-agent";

export function registerClearAlias(pi: ExtensionAPI): void {
	pi.registerCommand("clear", {
		description: "Start a new session (alias for /new)",
		handler: async (_args, ctx) => {
			await ctx.newSession();
		},
	});
}
