import * as path from "node:path";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { derivePhaseLabel } from "./lifecycle.js";
import { loadPreferences } from "./preferences.js";
import type { ScramjetState } from "./types.js";

const BELL_COOLDOWN_MS = 5_000;

const WAITING_PHASES = new Set(["idle", "waiting", "dormant"]);

export interface BellGuardArgs {
	bellEnabled: boolean;
	isTTY: boolean;
	isDispatchScheduled: boolean;
	isProbeScheduled: boolean;
	phase: string;
	lastBellMs: number;
	nowMs: number;
}

export function shouldRingBell(args: BellGuardArgs): boolean {
	if (!args.bellEnabled) return false;
	if (!args.isTTY) return false;
	if (args.isDispatchScheduled || args.isProbeScheduled) return false;
	if (!WAITING_PHASES.has(args.phase)) return false;
	if (args.nowMs - args.lastBellMs < BELL_COOLDOWN_MS) return false;
	return true;
}

export function titleForPhase(phase: string, sessionName: string | undefined, cwdBasename: string): string {
	const indicator = WAITING_PHASES.has(phase) ? "○" : "●";
	if (sessionName) return `${indicator} scramjet - ${sessionName} - ${cwdBasename}`;
	return `${indicator} scramjet - ${cwdBasename}`;
}

// Must be registered AFTER registerAutoContinue so auto-continue's agent_end
// fires first and updates lifecycle/timers before this handler reads them.
export function registerTerminalIndicators(pi: ExtensionAPI, state: ScramjetState): void {
	let lastBellMs = 0;

	function setTitleForPhase(ctx: { hasUI: boolean; ui: { setTitle(t: string): void } }, phase: string) {
		if (!ctx.hasUI) return;
		const prefs = loadPreferences(state.preferencesPath);
		if (!prefs.title_indicator) return;
		ctx.ui.setTitle(titleForPhase(phase, pi.getSessionName(), path.basename(process.cwd())));
	}

	pi.on("session_start", (_event, ctx) => {
		setTitleForPhase(ctx, "idle");
	});

	pi.on("agent_start", (_event, ctx) => {
		setTitleForPhase(ctx, "running");
	});

	pi.on("agent_end", (_event, ctx) => {
		const phase = derivePhaseLabel(state.lifecycle);
		setTitleForPhase(ctx, phase);

		const prefs = loadPreferences(state.preferencesPath);
		const isTTY = process.stdout.isTTY === true;
		const isDispatchScheduled = state.lifecycleTimers?.isDispatchScheduled() ?? false;
		const isProbeScheduled = state.lifecycleTimers?.isProbeScheduled() ?? false;
		const now = Date.now();

		if (
			shouldRingBell({
				bellEnabled: prefs.bell,
				isTTY,
				isDispatchScheduled,
				isProbeScheduled,
				phase,
				lastBellMs,
				nowMs: now,
			})
		) {
			process.stdout.write("\x07");
			lastBellMs = now;
		}
	});
}
