import type { ScramjetState } from "../types.ts";

export function freshState(overrides: Partial<ScramjetState> = {}): ScramjetState {
	return {
		enabled: false,
		registry: new Map(),
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
	handlers: Map<string, Handler[]>;
	emit: (event: string, payload?: unknown, ctx?: unknown) => Promise<void>;
}

// Recording Pi stub used across hook-driven tests. Captures every
// registerTool call and every on(event, handler) registration; `emit`
// fires all handlers for an event in registration order. Kept type-loose
// (`any` on `pi` and `tools`) so individual tests can adapt without
// fighting the type system.
export function recordingPi(): RecordingPi {
	const tools: any[] = [];
	const handlers = new Map<string, Handler[]>();
	const pi: any = {
		registerTool(tool: any) {
			tools.push(tool);
		},
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
	};
	async function emit(event: string, payload: unknown = {}, ctx: unknown = {}) {
		for (const h of handlers.get(event) ?? []) await h(payload, ctx);
	}
	return { pi, tools, handlers, emit };
}
