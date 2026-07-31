import { existsSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { join } from "node:path";
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
import { packageRoot } from "./docs-registry.js";
import { registerHistory } from "./history.js";
import { createLifecycle } from "./lifecycle.js";
import { createLogger } from "./logger.js";
import { registerModelChangeNotice } from "./model-change-notice.js";
import { registerModelIdentity } from "./model-identity.js";
import { registerModelSwitchTool } from "./model-switch-tool.js";
import { registerNextStepRecord } from "./next-step-record.js";
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
import { registerUpdateNotifier } from "./update-notifier.js";
import { registerUserInputTool } from "./user-input.js";

export interface RuntimeVersions {
	scramjet: string;
	agent: string;
	ai: string;
	codingAgent: string;
	tui: string;
}

export function readPackageVersion(metadataPath: string, packageName: string): string {
	try {
		const metadata: unknown = JSON.parse(readFileSync(metadataPath, "utf8"));
		if (!metadata || typeof metadata !== "object") return "unknown";
		const { name, version } = metadata as { name?: unknown; version?: unknown };
		return name === packageName && typeof version === "string" && version.trim() !== "" ? version : "unknown";
	} catch {
		return "unknown";
	}
}

function packageVersion(packageName: string): string {
	try {
		for (const root of createRequire(import.meta.url).resolve.paths(packageName) ?? []) {
			const metadataPath = join(root, packageName, "package.json");
			if (!existsSync(metadataPath)) continue;
			const version = readPackageVersion(metadataPath, packageName);
			if (version !== "unknown") return version;
		}
	} catch {
		return "unknown";
	}
	return "unknown";
}

export function runtimeVersions(): RuntimeVersions {
	return {
		scramjet: readPackageVersion(join(packageRoot(), "package.json"), "@leanandmean/scramjet"),
		agent: packageVersion("@leanandmean/agent"),
		ai: packageVersion("@leanandmean/ai"),
		codingAgent: packageVersion("@leanandmean/coding-agent"),
		tui: packageVersion("@leanandmean/tui"),
	};
}

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
		logger.debug("runtime", "runtime versions", { ...runtimeVersions() });
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
	registerNextStepRecord(pi);
	registerHistory(pi, state);
	registerPrIndicator(pi);
	registerUpdateNotifier(pi);
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
