export interface NextStep {
	command: string;
	freshSession: boolean;
	reason?: string;
}

export interface CompletionSignal {
	summary: string;
	nextStep?: NextStep;
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

export type CommandRegistry = Map<string, CommandDef>;

export interface AgentDef {
	name: string;
	filePath: string;
	description?: string;
}

export type AgentRegistry = Map<string, AgentDef>;

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
	// Set by the next-step dispatcher just before sendUserMessage when firing
	// a forced transition, so history's input handler can label the resulting
	// entry as origin: "forced" instead of "agent".
	pendingForcedDispatch: string | null;
}
