/**
 * Scramjet — smart auto-continuation for Pi. When a command finishes
 * and knows what should come next, scramjet just does it — unless you
 * stop it.
 *
 * Scramjet doesn't know about workflows. Each command independently
 * defines its own next step in its own instructions; the workflow is
 * whatever emerges from following those edges. No queue, no DAG, no
 * resumable state. Scramjet is invisible when it has nothing to
 * suggest.
 *
 * Pi extension entry point. Pi loads this file directly via jiti and
 * calls the default export with its ExtensionAPI. The function below
 * registers task_complete (the tool a command calls to signal
 * completion + optional next_step), the agent_end listener (drives
 * the countdown widget), draw_diagram (inline Mermaid/Graphviz/
 * PlantUML rendering), and the /scramjet on|off toggle. Install-time
 * concerns — the symlinks, the launcher shim, and the optional
 * pi-through-proxy models.json seed — live in install.sh.
 *
 * See README.md for the full pitch and CLAUDE.md for the design
 * principles that constrain what gets added here.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerAutoContinue } from "./auto-continue.ts";
import { registerClearAlias } from "./clear-alias.ts";
import { registerDiagramTool } from "./diagram/diagram-tool.ts";
import { registerScramjetCommand } from "./scramjet-command.ts";
import { registerToolAliases } from "./src/tool-aliases/index.ts";
import { registerTaskCompleteTool } from "./task-complete.ts";
import type { ScramjetState } from "./types.ts";

export default function scramjet(pi: ExtensionAPI) {
	const state: ScramjetState = {
		enabled: false,
		registry: new Map(),
		activeTopLevelCommand: null,
		sidebarLog: [],
		delegateStack: [],
	};

	registerTaskCompleteTool(pi, state);
	registerAutoContinue(pi, state);
	registerDiagramTool(pi);
	registerToolAliases(pi);
	registerScramjetCommand(pi, state);
	registerClearAlias(pi);
}
