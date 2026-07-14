import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { registerAgentCatalog } from "./agent-catalog.js";
import { registerAutoContinue } from "./auto-continue.js";
import { defaultConfigPath } from "./autonomy-settings.js";
import { registerAutopilotCommand } from "./autopilot-command.js";
import { registerBaseDirectives } from "./base-directives.js";
import { registerClearAlias } from "./clear-alias.js";
import { registerCommandCatalog } from "./command-catalog.js";
import { registerCommandStatusTool, registerDormantCommandNotice } from "./command-status.js";
import { registerCommandLoader } from "./commands/index.js";
import { registerDelegateTool } from "./delegate.js";
import { registerHistory } from "./history.js";
import { createLifecycle } from "./lifecycle.js";
import { createLogger } from "./logger.js";
import { registerModelChangeNotice } from "./model-change-notice.js";
import { registerModelIdentity } from "./model-identity.js";
import { registerModelSwitchTool } from "./model-switch-tool.js";
import { registerPrIndicator } from "./pr-indicator.js";
import { defaultPreferencesPath } from "./preferences.js";
import { registerScramjetCommand } from "./scramjet-command.js";
import { registerSubagentTool } from "./subagent/index.js";
import { registerSubagentOutputAdvisor } from "./subagent-output-advisor.js";
import { registerSubdirContext } from "./subdir-context.js";
import { registerSuggestNextStepsTool } from "./suggest-next-steps.js";
import { registerTerminalIndicators } from "./terminal-indicators.js";
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
		lifecycleGeneration: 0,
		pendingForcedDispatch: null,
		lifecycle: createLifecycle(),
		currentModel: null,
		modelHistory: [],
		pendingNotifyModel: null,
		hasUserMessage: false,
		autonomyConfigPath: defaultConfigPath(),
		autonomyRecommendations: new Map(),
		preferencesPath: defaultPreferencesPath(),
		subdirLoadedPaths: new Set(),
		pendingSuggestion: null,
		freetextAwaitingReply: false,
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
	registerSubagentTool(pi);
	registerAutopilotCommand(pi, state);
	registerScramjetCommand(pi, state);
	registerClearAlias(pi);
	registerCommandLoader(pi, state);
	registerModelIdentity(pi, state);
	registerModelSwitchTool(pi, state);
	registerModelChangeNotice(pi, state);
	registerHistory(pi, state);
	registerPrIndicator(pi);
	registerBaseDirectives(pi);
	registerAgentCatalog(pi, state);
	registerCommandCatalog(pi, state);
	registerDormantCommandNotice(pi, state);
	registerSuggestNextStepsTool(pi, state);
	registerSubdirContext(pi, state);
	// Must follow registerAutoContinue — hooks fire in registration order, and
	// indicators read lifecycle state set by auto-continue's agent_end handler.
	registerTerminalIndicators(pi, state);
}
