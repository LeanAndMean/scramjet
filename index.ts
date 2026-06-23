import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { registerAgentCatalog } from "./agent-catalog.ts";
import { registerAutoContinue } from "./auto-continue.ts";
import { defaultConfigPath } from "./autonomy-settings.ts";
import { registerBaseDirectives } from "./base-directives.ts";
import { registerClearAlias } from "./clear-alias.ts";
import { registerCommandStatusTool } from "./command-status.ts";
import { registerCommandLoader } from "./commands/index.ts";
import { registerDelegateTool } from "./delegate.ts";
import { registerDiagramTool } from "./diagram/diagram-tool.ts";
import { registerHistory } from "./history.ts";
import { createLogger } from "./logger.ts";
import { registerModelIdentity } from "./model-identity.ts";
import { registerPrIndicator } from "./pr-indicator.ts";
import { registerScramjetCommand } from "./scramjet-command.ts";
import { registerSubagentOutputAdvisor } from "./subagent-output-advisor.ts";
import { registerSubdirContext } from "./subdir-context.ts";
import { registerToolCallAdvisor } from "./tool-scope-advisory.ts";
import type { ScramjetState } from "./types.ts";
import { registerUserInputTool } from "./user-input.ts";

export default function scramjet(pi: ExtensionAPI) {
	const logger = createLogger(pi);
	const state: ScramjetState = {
		enabled: false,
		registry: new Map(),
		agentRegistry: new Map(),
		sidebarLog: [],
		delegateStack: [],
		pendingForcedDispatch: null,
		lifecycle: { phase: "idle" },
		currentModel: null,
		modelHistory: [],
		autonomyConfigPath: defaultConfigPath(),
		subdirLoadedPaths: new Set(),
		subdirDiscoveries: [],
		logger,
	};

	pi.on("session_start", (_event, ctx) => {
		logger.setHasUI(ctx.hasUI);
	});

	registerCommandStatusTool(pi, state);
	registerUserInputTool(pi, state);
	registerDelegateTool(pi, state);
	registerToolCallAdvisor(pi, state);
	registerSubagentOutputAdvisor(pi, state);
	registerAutoContinue(pi, state);
	registerDiagramTool(pi);
	registerScramjetCommand(pi, state);
	registerClearAlias(pi);
	registerCommandLoader(pi, state);
	registerModelIdentity(pi, state);
	registerHistory(pi, state);
	registerPrIndicator(pi);
	registerBaseDirectives(pi);
	registerAgentCatalog(pi, state);
	registerSubdirContext(pi, state);
}
