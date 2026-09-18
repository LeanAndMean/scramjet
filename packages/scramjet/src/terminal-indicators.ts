import * as path from "node:path";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { derivePhaseLabel } from "./lifecycle.js";
import { DEFAULT_PREFERENCES, loadPreferences, type Preferences } from "./preferences.js";
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

export type ChoiceCompletionDisposition = "resume-work" | "derive-lifecycle";

export interface ChoiceLease {
	complete(disposition: ChoiceCompletionDisposition): void;
}

export interface TerminalIndicatorContext {
	hasUI: boolean;
	ui: { setTitle(title: string): void };
}

export interface ChoiceIndicatorCoordinator {
	beginChoice(ctx: TerminalIndicatorContext): ChoiceLease;
}

export interface TerminalIndicatorCoordinator extends ChoiceIndicatorCoordinator {
	register(): void;
}

export function createTerminalIndicators(pi: ExtensionAPI, state: ScramjetState): TerminalIndicatorCoordinator {
	let lastBellMs = 0;
	let agentIsRunning = false;
	let deriveFromLifecycle = false;
	const activeChoices = new Set<symbol>();

	function safeLoadPreferences(): Preferences {
		try {
			return loadPreferences(state.preferencesPath);
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			state.logger.warn("preferences", msg);
			return { ...DEFAULT_PREFERENCES };
		}
	}

	function currentPhase(): string {
		if (activeChoices.size > 0) return "waiting";
		if (deriveFromLifecycle) return derivePhaseLabel(state.lifecycle);
		if (agentIsRunning) return "running";
		return derivePhaseLabel(state.lifecycle);
	}

	function titleProvider(): string | undefined {
		const prefs = safeLoadPreferences();
		if (!prefs.title_indicator) return undefined;
		return titleForPhase(currentPhase(), pi.getSessionName(), path.basename(process.cwd()));
	}

	function setTitleForPhase(ctx: TerminalIndicatorContext, phase: string, prefs: Preferences) {
		if (!ctx.hasUI) return;
		if (!prefs.title_indicator) return;
		ctx.ui.setTitle(titleForPhase(phase, pi.getSessionName(), path.basename(process.cwd())));
	}

	function setCurrentTitle(ctx: TerminalIndicatorContext): void {
		setTitleForPhase(ctx, currentPhase(), safeLoadPreferences());
	}

	function clearTransientState(): void {
		activeChoices.clear();
		deriveFromLifecycle = false;
	}

	function beginChoice(ctx: TerminalIndicatorContext): ChoiceLease {
		const id = Symbol();
		activeChoices.add(id);
		setCurrentTitle(ctx);
		return {
			complete(disposition) {
				if (!activeChoices.delete(id)) return;
				if (disposition === "derive-lifecycle") deriveFromLifecycle = true;
				setCurrentTitle(ctx);
			},
		};
	}

	// Must be registered AFTER registerAutoContinue so auto-continue's agent_end
	// fires first and updates lifecycle/timers before this handler reads them.
	function register(): void {
		pi.on("session_start", (_event, ctx) => {
			agentIsRunning = false;
			clearTransientState();
			if (ctx.hasUI) {
				ctx.ui.setTitleProvider(titleProvider);
			}
		});

		pi.on("session_tree", (_event, ctx) => {
			agentIsRunning = false;
			clearTransientState();
			setTitleForPhase(ctx, "idle", safeLoadPreferences());
		});

		pi.on("agent_start", (_event, ctx) => {
			clearTransientState();
			agentIsRunning = true;
			setTitleForPhase(ctx, "running", safeLoadPreferences());
		});

		pi.on("agent_end", (_event, ctx) => {
			agentIsRunning = false;
			clearTransientState();
			const prefs = safeLoadPreferences();
			const phase = derivePhaseLabel(state.lifecycle);
			setTitleForPhase(ctx, phase, prefs);

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

	return { beginChoice, register };
}
