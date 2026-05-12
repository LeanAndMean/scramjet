import { beforeEach, describe, expect, it } from "vitest";
import {
	clearLatestCompletion,
	getLatestCompletion,
	paramsToCompletionSignal,
	type TaskCompleteParams,
} from "../task-complete.ts";

describe("task-complete module state", () => {
	beforeEach(() => {
		clearLatestCompletion();
	});

	it("getLatestCompletion returns null after clearLatestCompletion", () => {
		expect(getLatestCompletion()).toBeNull();
	});
});

describe("paramsToCompletionSignal", () => {
	it("maps next_step snake_case fields to camelCase", () => {
		const params: TaskCompleteParams = {
			summary: "Implemented feature X",
			next_step: {
				command: "/run-tests",
				fresh_session: true,
				reason: "Verify the change",
			},
		};

		expect(paramsToCompletionSignal(params)).toEqual({
			summary: "Implemented feature X",
			nextStep: {
				command: "/run-tests",
				freshSession: true,
				reason: "Verify the change",
			},
		});
	});

	it("returns nextStep undefined when next_step is omitted", () => {
		const params: TaskCompleteParams = {
			summary: "Task done; nothing recommended next",
		};

		expect(paramsToCompletionSignal(params)).toEqual({
			summary: "Task done; nothing recommended next",
			nextStep: undefined,
		});
	});

	it("preserves command and freshSession when reason is omitted", () => {
		const params: TaskCompleteParams = {
			summary: "Stage 1 complete",
			next_step: {
				command: "/mach10:issue-implement 1 2",
				fresh_session: false,
			},
		};

		const signal = paramsToCompletionSignal(params);
		expect(signal.nextStep).toEqual({
			command: "/mach10:issue-implement 1 2",
			freshSession: false,
			reason: undefined,
		});
	});
});
