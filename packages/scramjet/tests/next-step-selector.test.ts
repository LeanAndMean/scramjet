import type { Api, Model } from "@leanandmean/ai";
import { describe, expect, it, vi } from "vitest";
import type { ValidatedNextStep } from "../src/commands/validator.js";
import { selectNextStep } from "../src/next-step-selector.js";

const ENTER = "\r";
const ESCAPE = "\x1b";
const ARROW_DOWN = "\x1b[B";
const ARROW_LEFT = "\x1b[D";
const ARROW_RIGHT = "\x1b[C";

function makeModel(provider: string, id: string, name?: string): Model<Api> {
	return {
		provider,
		id,
		name: name ?? `${provider}/${id}`,
		api: "anthropic-messages" as Api,
		baseUrl: "",
		reasoning: false,
		input: ["text"],
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
		contextWindow: 200000,
		maxTokens: 8192,
		headers: {},
	};
}

function makeStep(index: number, message: string, opts?: Partial<ValidatedNextStep>): ValidatedNextStep {
	return {
		index,
		message,
		reason: `reason for ${message}`,
		freshSession: false,
		parsedCommand: null,
		...opts,
	};
}

interface ComponentBag {
	components: any[];
	renderCalls: number;
}

function fakeCtx(): { ctx: any; bag: ComponentBag } {
	const bag: ComponentBag = { components: [], renderCalls: 0 };
	const ctx = {
		hasUI: true,
		ui: {
			custom<T>(factory: (tui: any, theme: any, keybindings: any, done: (result: T) => void) => any) {
				return new Promise<T>((resolve) => {
					let component: any;
					let settled = false;
					const done = (result: T) => {
						if (settled) return;
						settled = true;
						component?.dispose?.();
						resolve(result);
					};
					component = factory(
						{ requestRender: () => bag.renderCalls++ },
						{ fg: (_name: string, text: string) => text, bold: (text: string) => text },
						{},
						done,
					);
					bag.components.push(component);
				});
			},
		},
	};
	return { ctx, bag };
}

async function flush() {
	await Promise.resolve();
	await Promise.resolve();
	await Promise.resolve();
}

const modelA = makeModel("anthropic", "claude-sonnet-4-20250514", "Claude Sonnet 4");
const modelB = makeModel("openai", "gpt-4o", "GPT-4o");
const modelC = makeModel("anthropic", "claude-opus-4-20250514", "Claude Opus 4");

describe("selectNextStep — model cycling", () => {
	describe("without model cycling", () => {
		it("returns step with model: null when no models provided", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, { options: [step], recommended: step });
			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result).toEqual({ step, model: null });
		});

		it("returns null on escape", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, { options: [step], recommended: step });
			bag.components[0].handleInput(ESCAPE);
			await flush();

			expect(await p).toBeNull();
		});

		it("does not show model line when only 1 model available", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA],
				initialModel: modelA,
			});

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).not.toContain("model:");
			expect(rendered).not.toContain("←→");
			expect(rendered).toContain("↑↓ navigate");
		});

		it("does not show model line when models is empty", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [],
				initialModel: modelA,
			});

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).not.toContain("model:");
		});
	});

	describe("with model cycling", () => {
		it("renders model line and footer with ←→ hint", () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Claude Sonnet 4");
			expect(rendered).toContain("anthropic/claude-sonnet-4-20250514");
			expect(rendered).toContain("←/→ change");
			expect(rendered).toContain("←→ model");
		});

		it("default Enter returns model: null (no cycling)", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result).toEqual({ step, model: null });
		});

		it("right arrow cycles to next model", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB, modelC],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ARROW_RIGHT);
			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: GPT-4o");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBe(modelB);
		});

		it("left arrow cycles to previous model (wraps)", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB, modelC],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ARROW_LEFT);
			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Claude Opus 4");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBe(modelC);
		});

		it("right arrow wraps from last to first model", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: modelB,
			});

			bag.components[0].handleInput(ARROW_RIGHT);
			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Claude Sonnet 4");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBe(modelA);
		});

		it("cycling away and back returns model: null", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ARROW_RIGHT); // -> modelB
			bag.components[0].handleInput(ARROW_LEFT); // -> back to modelA

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBeNull();
		});

		it("escape after cycling returns null", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ARROW_RIGHT);
			bag.components[0].handleInput(ESCAPE);
			await flush();

			expect(await p).toBeNull();
		});

		it("up/down still navigate options, not models", async () => {
			const { ctx, bag } = fakeCtx();
			const step0 = makeStep(0, "/test:first");
			const step1 = makeStep(1, "/test:second");

			const p = selectNextStep(ctx, {
				options: [step0, step1],
				recommended: step0,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			bag.components[0].handleInput(ARROW_DOWN);
			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.step).toBe(step1);
			expect(result!.model).toBeNull();
		});

		it("left/right clears countdown", async () => {
			vi.useFakeTimers();
			try {
				const { ctx, bag } = fakeCtx();
				const step = makeStep(0, "/test:cmd");

				selectNextStep(ctx, {
					options: [step],
					recommended: step,
					autoSelect: step,
					countdownSeconds: 10,
					models: [modelA, modelB],
					initialModel: modelA,
				});

				const rendered1 = bag.components[0].render(80).join("\n");
				expect(rendered1).toContain("auto-selects");

				bag.components[0].handleInput(ARROW_RIGHT);

				// Advance past original countdown — should not auto-select
				await vi.advanceTimersByTimeAsync(15000);
				await flush();

				const rendered2 = bag.components[0].render(80).join("\n");
				expect(rendered2).not.toContain("auto-selects");
			} finally {
				vi.useRealTimers();
			}
		});

		it("countdown auto-select returns model: null", async () => {
			vi.useFakeTimers();
			try {
				const { ctx } = fakeCtx();
				const step = makeStep(0, "/test:cmd");

				const p = selectNextStep(ctx, {
					options: [step],
					recommended: step,
					autoSelect: step,
					countdownSeconds: 3,
					models: [modelA, modelB],
					initialModel: modelA,
				});

				await vi.advanceTimersByTimeAsync(4000);
				await flush();

				const result = await p;
				expect(result).toEqual({ step, model: null });
			} finally {
				vi.useRealTimers();
			}
		});
	});

	describe("off-list current model (sentinel)", () => {
		const offListModel = makeModel("custom", "off-list-model", "Off-List Model");

		it("displays off-list model as current", () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: offListModel,
			});

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Off-List Model");
			expect(rendered).toContain("custom/off-list-model");
		});

		it("cycling right from off-list goes to first available model", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: offListModel,
			});

			bag.components[0].handleInput(ARROW_RIGHT);
			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Claude Sonnet 4");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBe(modelA);
		});

		it("cycling left from off-list goes to last available model", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: offListModel,
			});

			bag.components[0].handleInput(ARROW_LEFT);
			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: GPT-4o");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBe(modelB);
		});

		it("cycling away and back to sentinel returns model: null", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			const p = selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: offListModel,
			});

			bag.components[0].handleInput(ARROW_RIGHT); // -> modelA
			bag.components[0].handleInput(ARROW_LEFT); // -> back to sentinel

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Off-List Model");

			bag.components[0].handleInput(ENTER);
			await flush();

			const result = await p;
			expect(result!.model).toBeNull();
		});

		it("wraps right from last model back to sentinel", async () => {
			const { ctx, bag } = fakeCtx();
			const step = makeStep(0, "/test:cmd");

			selectNextStep(ctx, {
				options: [step],
				recommended: step,
				models: [modelA, modelB],
				initialModel: offListModel,
			});

			bag.components[0].handleInput(ARROW_RIGHT); // -> modelA (idx 0)
			bag.components[0].handleInput(ARROW_RIGHT); // -> modelB (idx 1)
			bag.components[0].handleInput(ARROW_RIGHT); // -> sentinel (-1)

			const rendered = bag.components[0].render(80).join("\n");
			expect(rendered).toContain("model: Off-List Model");
		});
	});

	describe("aborted signal", () => {
		it("returns null when signal is pre-aborted (with models)", async () => {
			const { ctx } = fakeCtx();
			const step = makeStep(0, "/test:cmd");
			const controller = new AbortController();
			controller.abort();

			const result = await selectNextStep(ctx, {
				options: [step],
				recommended: step,
				signal: controller.signal,
				models: [modelA, modelB],
				initialModel: modelA,
			});

			expect(result).toBeNull();
		});
	});
});
