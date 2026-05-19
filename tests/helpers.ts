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
