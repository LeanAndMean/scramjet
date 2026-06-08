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
//   reported — the agent answered the probe via scramjet_command_status
export type CommandPhase = "idle" | "running" | "probing" | "reported";

// A single next-step suggestion in a command-status report. This is the
// tool-facing (snake_case) shape the agent populates in the scramjet_command_status
// next_steps payload; auto-continue converts the chosen entry into a NextStep
// before dispatch.
export interface CommandStatusNextStep {
	name: string;
	args?: string;
	fresh_session: boolean;
	label?: string;
	reason?: string;
}

// Structured result the agent supplies through scramjet_command_status in
// response to the post-response status probe. `next_steps` is an array (not a
// singular pick) so it can carry candidates for the future choice-list UI;
// the MVP auto-continue dispatches the first valid entry.
export interface CommandStatusPayload {
	status: "completed" | "waiting_for_user" | "blocked" | "incomplete";
	summary: string;
	user_prompt?: string;
	next_steps?: CommandStatusNextStep[];
}

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
	// scramjet_command_status report, read by auto-continue on the probe turn's
	// agent_end. Reset to "idle"/null on command start and on resume/rebuild.
	commandPhase: CommandPhase;
	latestCommandStatus: CommandStatusPayload | null;
}
