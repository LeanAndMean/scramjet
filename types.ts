export interface ScramjetState {
	enabled: boolean;
}

export interface NextStep {
	command: string;
	freshSession: boolean;
	reason?: string;
}

export interface CompletionSignal {
	summary: string;
	nextStep?: NextStep;
}
