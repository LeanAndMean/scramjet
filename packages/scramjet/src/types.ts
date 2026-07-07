import type { LifecycleHolder } from "./lifecycle.js";

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
	// Only meaningful for slash commands.
	fresh_session: boolean;
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
	argumentHint?: string;
	delegateOnly?: true;
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

// Name of the harness-only tool that records a user-initiated model change as a real
// tool row (issue 244). Lives here (not in model-change-notice.ts, which owns the tool)
// so model-identity.ts can match persisted notice entries without a circular import.
export const MODEL_CHANGE_NOTICE_TOOL = "scramjet_model_change_notice";

export interface ModelRecord {
	name: string;
	id: string;
	provider: string;
	fromTurnIndex: number;
}

export interface PendingSuggestion {
	steps: [CommandStatusNextStep, ...CommandStatusNextStep[]];
	recommendedIndex?: number;
	generation: number;
}

// Exposed for test observability of closure-local timer state in auto-continue.ts.
export interface LifecycleTimerAccessors {
	isProbeScheduled(): boolean;
	isWatchdogActive(): boolean;
	isDispatchScheduled(): boolean;
}

export interface ScramjetState extends LifecycleHolder {
	enabled: boolean;
	registry: CommandRegistry;
	agentRegistry: AgentRegistry;
	sidebarLog: SidebarEntry[];
	delegateStack: DelegateFrame[];
	clearLifecycleTimers?: (reason?: string) => void;
	// Set by the next-step dispatcher just before slash-input dispatch when
	// firing a forced transition, so history's input handler can label the
	// resulting entry as origin: "forced" instead of "agent".
	pendingForcedDispatch: string | null;
	currentModel: ModelRecord | null;
	modelHistory: ModelRecord[];
	// Set by switch_scramjet_model just before pi.setModel (issue 244, Stage 4) so
	// the model_select handler (Stage 5) can skip emitting a user-change notice for
	// an agent-initiated switch. Read and cleared synchronously inside setModel's
	// model_select emission by model-change-notice.ts.
	suppressNextModelNotify?: boolean;
	// A user-initiated model change (issue 244, Stage 5) whose scramjet_model_change_notice
	// delivery is deferred because a probe is armed/in-flight. Holds only the latest
	// pending model (structural coalescing: intermediate models never reach delivery)
	// and is drained on the next non-probe agent_end. Owned by model-change-notice.ts.
	pendingNotifyModel: ModelRecord | null;
	// True once the first user message exists this session (issue 244). Gates the
	// pre-first-turn boundary: before the first user message a model change updates the
	// system prompt's # Model Identity section directly and fires no notice tool; after
	// it, changes deliver via scramjet_model_change_notice. Latched live by
	// model-change-notice.ts's `input` observer, and re-derived from the branch on
	// resume/fork/session-tree by model-identity.ts's rebuild (Stage 6), so a resumed
	// session past its first user message stays past the boundary.
	hasUserMessage: boolean;
	lifecycleTimers?: LifecycleTimerAccessors;
	suspendProbeWatchdog?: () => void;
	rearmProbeWatchdog?: () => void;
	autonomyConfigPath: string;
	subdirLoadedPaths: Set<string>;
	pendingSuggestion: PendingSuggestion | null;
	freetextAwaitingReply: boolean;
}
