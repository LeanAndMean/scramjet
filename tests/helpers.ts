import type { LifecycleState } from "../phase-machine.ts";
import type { ScramjetState } from "../types.ts";

export function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
		enabled: false,
		registry: new Map(),
		agentRegistry: new Map(),
		sidebarLog: [],
		delegateStack: [],
		pendingForcedDispatch: null,
		lifecycle: { phase: "idle" },
		suspendProbeWatchdog: undefined,
		rearmProbeWatchdog: undefined,
		autonomyConfigPath: "/tmp/scramjet-test/autonomy.yaml",
		...overrides,
	};
}

/**
 * Creates a lifecycle state for a given phase, providing sensible defaults.
 * Use this in tests that previously passed `commandPhase` / `activeTopLevelCommand`.
 */
export function lifecycleFor(
	phase: LifecycleState["phase"],
	command = "test:cmd",
	extra?: { continueCount?: number; status?: import("../types.ts").CommandStatusRestingPayload },
): LifecycleState {
	switch (phase) {
		case "idle":
			return { phase: "idle" };
		case "dormant":
			return { phase: "dormant", command };
		case "running":
			return { phase: "running", command, continueCount: extra?.continueCount ?? 0 };
		case "probing":
			return { phase: "probing", command, continueCount: extra?.continueCount ?? 0 };
		case "reported":
			return {
				phase: "reported",
				command,
				status: extra?.status ?? { status: "completed", summary: "done" },
				continueCount: extra?.continueCount ?? 0,
			};
		case "waiting":
			return { phase: "waiting", command };
	}
}

type Handler = (event: unknown, ctx?: unknown) => unknown;

export interface RecordingPi {
	pi: any;
	tools: any[];
	commands: { name: string; spec: unknown }[];
	handlers: Map<string, Handler[]>;
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

// Recording Pi stub used across hook-driven tests. Captures every
// registerTool / registerCommand call and every on(event, handler)
// registration; `emit` fires all handlers for an event in registration order.
// Kept type-loose (`any` on `pi` and `tools`) so individual tests can adapt
// without fighting the type system.
export function recordingPi(): RecordingPi {
	const tools: any[] = [];
	const commands: { name: string; spec: unknown }[] = [];
	const handlers = new Map<string, Handler[]>();
	const pi: any = {
		appended: [] as { customType: string; data: unknown }[],
		// Records pi.sendMessage(message, options) calls. The two-phase
		// command-status protocol (issue 84) sends the hidden status probe this
		// way; tests assert it was sent (and, for the F1 deferral, that it fired
		// off the timer rather than synchronously inside agent_end).
		sent: [] as { message: unknown; options?: unknown }[],
		// Messages dropped because they were sent while the run was still
		// streaming. The real harness drops a sendMessage issued from inside an
		// agent_end listener (isStreaming === true until the run settles — it clears
		// when agent.prompt() resolves), so a synchronous probe would never reach the
		// model. A
		// test that models this catches a regression that sends the probe inline
		// rather than deferring it past the streaming window.
		dropped: [] as { message: unknown; options?: unknown }[],
		isStreaming: false,
		registerTool(tool: any) {
			tools.push(tool);
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
