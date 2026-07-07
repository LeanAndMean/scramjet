import { createLifecycle, type LifecycleHolder, type LifecycleState } from "../src/lifecycle.js";
import { createLogger, SCRAMJET_LOG_TYPE } from "../src/logger.js";
import type { CommandStatusRestingPayload, ScramjetState } from "../src/types.js";

export function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
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
		suppressNextModelNotify: false,
		suspendProbeWatchdog: undefined,
		rearmProbeWatchdog: undefined,
		autonomyConfigPath: "/tmp/scramjet-test/autonomy.yaml",
		subdirLoadedPaths: new Set<string>(),
		pendingSuggestion: null,
		freetextAwaitingReply: false,
		logger: createLogger({ appendEntry() {} } as any),
		...overrides,
	};
}

/**
 * Creates a lifecycle state for a given phase name, providing sensible defaults.
 * Maps phase names to lifecycle facts for test convenience.
 */
export function lifecycleFor(
	phase: "idle" | "dormant" | "running" | "probing" | "reported" | "waiting",
	command = "test:cmd",
	extra?: { continueCount?: number; status?: CommandStatusRestingPayload },
): LifecycleState {
	const lc = createLifecycle();
	switch (phase) {
		case "idle":
			return lc;
		case "dormant":
			lc.activeCommand = command;
			return lc;
		case "running":
			lc.activeCommand = command;
			lc.probeArmed = true;
			lc.continueCount = extra?.continueCount ?? 0;
			return lc;
		case "probing":
			lc.activeCommand = command;
			lc.probeInFlight = true;
			lc.continueCount = extra?.continueCount ?? 0;
			return lc;
		case "reported":
			lc.activeCommand = command;
			lc.lastReport = extra?.status ?? { status: "completed", summary: "done" };
			return lc;
		case "waiting":
			lc.activeCommand = command;
			lc.parkedForInput = true;
			return lc;
	}
}

export function freshLifecycleHolder(overrides: Partial<LifecycleState> = {}): LifecycleHolder {
	return {
		lifecycle: { ...createLifecycle(), ...overrides },
		lifecycleGeneration: 0,
		logger: createLogger({ appendEntry() {} } as any),
	};
}

type Handler = (event: unknown, ctx?: unknown) => unknown;

export interface RecordingPi {
	pi: any;
	tools: any[];
	commands: { name: string; spec: unknown }[];
	handlers: Map<string, Handler[]>;
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

export function recordingPi(): RecordingPi {
	const tools: any[] = [];
	const commands: { name: string; spec: unknown }[] = [];
	const handlers = new Map<string, Handler[]>();
	const pi: any = {
		appended: [] as { customType: string; data: unknown }[],
		sent: [] as { message: unknown; options?: unknown }[],
		dropped: [] as { message: unknown; options?: unknown }[],
		harnessToolCalls: [] as { name: string; args: unknown; options?: unknown }[],
		setModelCalls: [] as { model: unknown; result: boolean }[],
		setModelResult: true as boolean | Error,
		isStreaming: false,
		registerTool(tool: any) {
			tools.push(tool);
		},
		async invokeHarnessTool(name: string, args: unknown, options?: unknown) {
			pi.harnessToolCalls.push({ name, args, options });
		},
		async setModel(model: unknown): Promise<boolean> {
			if (pi.setModelResult instanceof Error) {
				pi.setModelCalls.push({ model, result: false });
				throw pi.setModelResult;
			}
			pi.setModelCalls.push({ model, result: pi.setModelResult });
			return pi.setModelResult;
		},
		registerCommand(name: string, spec: unknown) {
			commands.push({ name, spec });
		},
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
		appendEntry(customType: string, data: unknown) {
			pi.appended.push({ customType, data });
		},
		sendMessage(message: unknown, options?: unknown) {
			if (pi.isStreaming) {
				pi.dropped.push({ message, options });
				return;
			}
			pi.sent.push({ message, options });
		},
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, tools, commands, handlers, emit };
}

export { derivePhaseLabel as derivedPhase } from "../src/lifecycle.js";

export function logMessages(pi: any, level?: string): string[] {
	return pi.appended
		.filter((entry: any) => entry.customType === SCRAMJET_LOG_TYPE && (level ? entry.data.level === level : true))
		.map((entry: any) => entry.data.message);
}
