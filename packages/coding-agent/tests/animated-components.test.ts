import type { TUI } from "@leanandmean/tui";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ArminComponent } from "../src/modes/interactive/components/armin.js";
import { DaxnutsComponent } from "../src/modes/interactive/components/daxnuts.js";

describe("animated component completion", () => {
	const ui = { requestRender: vi.fn() } as unknown as TUI;

	beforeEach(() => {
		vi.useFakeTimers();
		vi.spyOn(Math, "random").mockReturnValue(0);
	});

	afterEach(() => {
		vi.restoreAllMocks();
		vi.useRealTimers();
	});

	it("supports constructing ArminComponent without a completion callback", async () => {
		new ArminComponent(ui);

		await vi.runAllTimersAsync();
	});

	it("delivers ArminComponent completion callbacks", async () => {
		const onComplete = vi.fn();
		new ArminComponent(ui, onComplete);

		await vi.runAllTimersAsync();
		expect(onComplete).toHaveBeenCalledOnce();
	});

	it("supports constructing DaxnutsComponent without a completion callback", async () => {
		new DaxnutsComponent(ui);

		await vi.runAllTimersAsync();
	});

	it("delivers DaxnutsComponent completion callbacks", async () => {
		const onComplete = vi.fn();
		new DaxnutsComponent(ui, onComplete);

		await vi.runAllTimersAsync();
		expect(onComplete).toHaveBeenCalledOnce();
	});
});
