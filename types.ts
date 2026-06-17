import type { LifecycleState } from "./phase-machine.ts";

export interface NextStep {
	// Bare command name (no leading slash, no args). Matches against
	// candidate names for closed/open validation. The dispatcher in
	// auto-continue.ts owns the slash prefix and the args join, so the same
	// `name` value is what the validator sees and what the wire payload is
	// built from. (F15)
	name: string;
	// Optional space-prefixed args. Carried verbatim into the wire payload
	// after the slash + name; never inspected by the validator.
	args?: string;
	freshSession: boolean;
	reason?: string;
}

// A single next-step suggestion in a command-status report: a suggested next
// message. `message` is both the selector display text and the dispatched
// payload — a leading slash makes it a command (auto-dispatched on selection);
// anything else is pasted into the editor. The harness parses the `/` prefix
// itself (commands/validator.ts parseSlashCommand); the agent never declares a
// type discriminator, and there is no label indirection — the user sees
// exactly what will run.
export interface CommandStatusNextStep {
	// Displayed in the selector AND dispatched on selection.
	message: string;
	// Only meaningful for slash commands; defaults to false.
	fresh_session?: boolean;
	// Shown as the description underneath the message.
	reason?: string;
}

// Structured result the agent supplies through report_scramjet_command_status in
// response to the post-response status probe. `recommended_next_step` is a
// zero-based index into the original `next_steps` array.
export interface CommandStatusPayload {
	status: "completed" | "blocked" | "incomplete" | "continuing";
	summary: string;
	next_steps?: CommandStatusNextStep[];
	recommended_next_step?: number;
}

export type CommandStatusRestingStatus = Exclude<CommandStatusPayload["status"], "continuing">;
export type CommandStatusRestingPayload = Omit<CommandStatusPayload, "status"> & {
	status: CommandStatusRestingStatus;
};

export interface Candidate {
	name: string;
	hint?: string;
}

export type NextStepPolicy =
	| { mode: "forced"; target: string }
	| { mode: "closed"; candidates: Candidate[] }
	| { mode: "open"; candidates: Candidate[]; blacklist?: string[] }
	| { mode: "ask"; hint?: string };

export interface CommandDef {
	name: string;
	filePath: string;
	body: string;
	description?: string;
	allowedTools?: string[];
	next?: NextStepPolicy;
}

export type CommandRegistry = ReadonlyMap<string, CommandDef>;

export interface AgentDef {
	name: string;
	filePath: string;
	description?: string;
}

export type AgentRegistry = ReadonlyMap<string, AgentDef>;

export interface DelegateFrame {
	commandName: string;
	effectiveAllowedTools?: string[];
	depth: number;
}

export interface SidebarEntry {
	command: string;
	origin: "user" | "agent" | "forced";
	depth: number;
	timestamp: number;
}

export type EdgeSetting = "chain" | "pause" | null;

export interface AutonomyConfig {
	edges: Record<string, Record<string, NonNullable<EdgeSetting>>>;
}

// Exposed for test observability of closure-local timer state in auto-continue.ts.
export interface LifecycleTimerAccessors {
	isProbeScheduled(): boolean;
	isWatchdogActive(): boolean;
	isDispatchScheduled(): boolean;
}

export interface ScramjetState {
	enabled: boolean;
	registry: CommandRegistry;
	agentRegistry: AgentRegistry;
	sidebarLog: SidebarEntry[];
	delegateStack: DelegateFrame[];
	// Set by the next-step dispatcher just before slash-input dispatch when
	// firing a forced transition, so history's input handler can label the
	// resulting entry as origin: "forced" instead of "agent".
	pendingForcedDispatch: string | null;
	lifecycle: LifecycleState;
	lifecycleTimers?: LifecycleTimerAccessors;
	suspendProbeWatchdog?: () => void;
	rearmProbeWatchdog?: () => void;
	autonomyConfigPath: string;
}
