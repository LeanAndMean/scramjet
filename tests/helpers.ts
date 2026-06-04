import type { ScramjetState } from "../types.ts";

export function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
		enabled: false,
		registry: new Map(),
		agentRegistry: new Map(),
		activeTopLevelCommand: null,
		sidebarLog: [],
		delegateStack: [],
		pendingForcedDispatch: null,
		...overrides,
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
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, tools, commands, handlers, emit };
}
