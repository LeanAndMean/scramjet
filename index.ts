/**
 * Scramjet - Smart auto-continuation for Pi
 *
 * When a command completes and suggests a next step, Scramjet shows a
 * countdown and auto-runs it. The user can press Escape or type anything
 * to cancel. Invisible when there's nothing to suggest.
 *
 * Also provides a draw_diagram tool for inline Mermaid/Graphviz/PlantUML rendering.
 *
 * Install: symlink or copy to ~/.pi/agent/extensions/scramjet/
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerAutoContinue } from "./auto-continue.ts";
import { registerDiagramTool } from "./diagram/diagram-tool.ts";
import { registerScramjetCommand } from "./scramjet-command.ts";
import { registerTaskCompleteTool } from "./task-complete.ts";

export default function scramjet(pi: ExtensionAPI) {
	const state = { enabled: true };

	registerTaskCompleteTool(pi, state);
	registerAutoContinue(pi, state);
	registerDiagramTool(pi);
	registerScramjetCommand(pi, state);
}
