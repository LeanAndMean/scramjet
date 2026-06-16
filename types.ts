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

// Lifecycle phase of the active top-level Scramjet command, tracked per
// invocation by the two-phase command-status protocol (issue 84):
//   idle     — no active command, or the chain has been resolved
//   running  — the command's normal answer turn is in flight
//   probing  — the answer turn ended; Scramjet has asked for a status check
//   reported — the agent answered the probe via report_scramjet_command_status
//   waiting  — the probe reported waiting_for_user; the command is paused for
//              input but stays associated with its invocation. A later
//              interactive reply re-arms the running→probing probe path so an
//              interactive command can resume and report completed (issue 88).
//              The only resting phase besides idle. Survives rewind/resume:
//              replayHistory reconstructs "waiting" from the journaled
//              COMMAND_STATUS_TYPE entries (history.ts, issue 88 Stage 2).
export type CommandPhase = "idle" | "running" | "probing" | "reported" | "waiting";

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
	status: "completed" | "waiting_for_user" | "blocked" | "incomplete" | "continuing";
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusNextStep[];
	recommended_next_step?: number;
}

export type CommandStatusRestingStatus = Exclude<CommandStatusPayload["status"], "continuing">;

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
	edges: Record<string, Record<string, "chain" | "pause">>;
}

export interface ScramjetState {
	enabled: boolean;
	registry: CommandRegistry;
	agentRegistry: AgentRegistry;
	activeTopLevelCommand: string | null;
	sidebarLog: SidebarEntry[];
	delegateStack: DelegateFrame[];
	// Set by the next-step dispatcher just before slash-input dispatch when
	// firing a forced transition, so history's input handler can label the
	// resulting entry as origin: "forced" instead of "agent".
	pendingForcedDispatch: string | null;
	// Two-phase command-status protocol (issue 84). commandPhase tracks the
	// lifecycle of the active top-level command so the probe fires exactly
	// once per invocation; latestCommandStatus holds the agent's most recent
	// report_scramjet_command_status report, read by auto-continue on the probe turn's
	// agent_end. Reset to "idle"/null on command start and on resume/rebuild.
	commandPhase: CommandPhase;
	latestCommandStatus: CommandStatusPayload | null;
	suspendProbeWatchdog?: () => void;
	rearmProbeWatchdog?: () => void;
	resetConsecutiveContinues?: () => void;
	autonomyConfigPath: string;
}
