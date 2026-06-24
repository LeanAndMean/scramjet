import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { registerAgentCatalog } from "./agent-catalog.js";
import { registerAutoContinue } from "./auto-continue.js";
import { defaultConfigPath } from "./autonomy-settings.js";
import { registerBaseDirectives } from "./base-directives.js";
import { registerClearAlias } from "./clear-alias.js";
import { registerCommandStatusTool } from "./command-status.js";
import { registerCommandLoader } from "./commands/index.js";
import { registerDelegateTool } from "./delegate.js";
import { registerDiagramTool } from "./diagram/diagram-tool.js";
import { registerHistory } from "./history.js";
import { createLogger } from "./logger.js";
import { registerModelIdentity } from "./model-identity.js";
import { registerPrIndicator } from "./pr-indicator.js";
import { registerScramjetCommand } from "./scramjet-command.js";
import { registerSubagentOutputAdvisor } from "./subagent-output-advisor.js";
import { registerSubdirContext } from "./subdir-context.js";
import { registerToolCallAdvisor } from "./tool-scope-advisory.js";
import type { ScramjetState } from "./types.js";
import { registerUserInputTool } from "./user-input.js";

export function initScramjet(pi: ExtensionAPI) {
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
