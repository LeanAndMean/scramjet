/**
 * Scramjet — smart auto-continuation and command-set harness for Pi.
 *
 * Scramjet doesn't know about workflows. Each command independently
 * declares its own next step in YAML frontmatter; the workflow is
 * whatever emerges from following those edges. No queue, no DAG, no
 * resumable state. Scramjet is invisible when it has nothing to
 * suggest.
 *
 * Pi extension entry point. Distributed as an npm package
 * (@leanandmean/scramjet); bin/scramjet.js imports the default export
 * below and hands it to Pi via main()'s extensionFactories option. The
 * function registers the tools the harness owns (task_complete,
 * delegate, and draw_diagram when a renderer is available), the
 * agent_end listener (drives the countdown widget
 * and next-step dispatch), command-set discovery, history journaling,
 * advisory tool-scope warnings, draw_diagram, and the /scramjet on|off
 * toggle.
 *
 * See README.md for the pitch and CLAUDE.md for the design principles
 * that constrain what gets added here.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerAutoContinue } from "./auto-continue.ts";
import { registerClearAlias } from "./clear-alias.ts";
import { registerCommandLoader } from "./commands/index.ts";
import { registerDelegateTool } from "./delegate.ts";
import { registerDiagramTool } from "./diagram/diagram-tool.ts";
import { registerHistory } from "./history.ts";
import { registerScramjetCommand } from "./scramjet-command.ts";
import { registerSubagentOutputAdvisor } from "./subagent-output-advisor.ts";
import { registerTaskCompleteTool } from "./task-complete.ts";
import { registerToolCallAdvisor } from "./tool-scope-advisory.ts";
import type { ScramjetState } from "./types.ts";

export default function scramjet(pi: ExtensionAPI) {
	const state: ScramjetState = {
		enabled: false,
		registry: new Map(),
		agentRegistry: new Map(),
		activeTopLevelCommand: null,
		sidebarLog: [],
		delegateStack: [],
		pendingForcedDispatch: null,
	};

	registerTaskCompleteTool(pi, state);
	registerDelegateTool(pi, state);
	registerToolCallAdvisor(pi, state);
	registerSubagentOutputAdvisor(pi, state);
	registerAutoContinue(pi, state);
	registerDiagramTool(pi);
	registerScramjetCommand(pi, state);
	registerClearAlias(pi);
	registerCommandLoader(pi, state);
	registerHistory(pi, state);
}
