import { type Api, type Model, modelsAreEqual } from "@leanandmean/ai";
import type { ExtensionContext } from "@leanandmean/coding-agent";
import { getKeybindings } from "@leanandmean/tui";
import type { ValidatedNextStep } from "./commands/validator.js";
import { MultiLineSelectList } from "./multi-line-select.js";

export interface ScramjetSelectorOption {
	index: number;
	reason: string;
}

export interface ScramjetSelectorOptions<TOption extends ScramjetSelectorOption> {
	title: string;
	options: TOption[];
	recommended: TOption | null;
	getTitle(option: TOption): string;
	getDescription(option: TOption): string;
	autoSelect?: TOption;
	countdownSeconds?: number;
	signal?: AbortSignal;
}

export interface NextStepSelection {
	step: ValidatedNextStep;
	model: Model<any> | null;
}

export interface NextStepSelectorOptions {
	options: ValidatedNextStep[];
	recommended: ValidatedNextStep | null;
	autoSelect?: ValidatedNextStep;
	countdownSeconds?: number;
	signal?: AbortSignal;
	models?: Model<Api>[];
	initialModel?: Model<any>;
}

function cleanDisplay(text: string): string {
	return text
		.replace(/[\u0000-\u001f\u007f]/g, " ")
		.trim()
		.replace(/\s+/g, " ");
}

function optionTitle(option: ValidatedNextStep): string {
	// What you see is what you get: the message is both the display text and
	// the dispatched payload. No label indirection.
	return cleanDisplay(option.message);
}

export async function selectScramjetChoice<TOption extends ScramjetSelectorOption>(
	ctx: ExtensionContext,
	{
		title,
		options,
		recommended,
		getTitle,
		getDescription,
		autoSelect,
		countdownSeconds = 0,
		signal,
	}: ScramjetSelectorOptions<TOption>,
): Promise<TOption | null> {
	if (signal?.aborted) return null;

	let fail: (err: unknown) => void = () => {};
	const failure = new Promise<never>((_resolve, reject) => {
		fail = reject;
	});
	const byValue = new Map(options.map((option) => [String(option.index), option]));
	const items = options.map((option) => ({
		value: String(option.index),
		label: `${option.index}: ${getTitle(option)}`,
		description: getDescription(option),
	}));

	const recommendedIndex = recommended ? options.findIndex((option) => option.index === recommended.index) : -1;
	const selectedValue = await Promise.race([
		ctx.ui.custom<string | null>((tui, theme, _keybindings, done) => {
			let remaining = countdownSeconds;
			let timer: ReturnType<typeof setInterval> | null = null;
			let finished = false;

			const selectList = new MultiLineSelectList(
				items,
				Math.min(items.length, 8),
				{
					selectedText: (text) => theme.fg("accent", text),
					description: (text) => theme.fg("muted", text),
					scrollInfo: (text) => theme.fg("dim", text),
				},
				{ recommendedIndex },
			);

			if (recommendedIndex >= 0) selectList.setSelectedIndex(recommendedIndex);

			const abort = () => finish(null);

			function clearTimer() {
				if (timer) {
					clearInterval(timer);
					timer = null;
				}
			}

			function finish(value: string | null) {
				if (finished) return;
				finished = true;
				clearTimer();
				signal?.removeEventListener("abort", abort);
				done(value);
			}

			selectList.onSelect = (item) => finish(item.value);
			selectList.onCancel = () => finish(null);

			signal?.addEventListener("abort", abort, { once: true });
			if (signal?.aborted) finish(null);

			if (!finished && autoSelect && remaining > 0) {
				timer = setInterval(() => {
					try {
						remaining--;
						if (remaining <= 0) {
							finish(String(autoSelect.index));
						} else {
							tui.requestRender();
						}
					} catch (err) {
						fail(err);
						finish(null);
					}
				}, 1000);
			}

			return {
				render(width: number) {
					const footer = timer
						? `↑↓ navigate • enter select • esc cancel • auto-selects recommendation in ${remaining}s`
						: "↑↓ navigate • enter select • esc cancel";
					return [theme.fg("accent", theme.bold(title)), ...selectList.render(width), theme.fg("dim", footer)];
				},
				invalidate() {
					selectList.invalidate();
				},
				handleInput(data: string) {
					try {
						clearTimer();
						selectList.handleInput(data);
						tui.requestRender();
					} catch (err) {
						fail(err);
						finish(null);
					}
				},
				dispose() {
					clearTimer();
				},
			};
		}),
		failure,
	]);

	if (selectedValue === null) return null;

	const selected = byValue.get(selectedValue);
	if (!selected) throw new Error(`selector returned unknown option value ${JSON.stringify(selectedValue)}`);
	return selected;
}

export function selectNextStep(
	ctx: ExtensionContext,
	{ options, recommended, autoSelect, countdownSeconds = 0, signal, models, initialModel }: NextStepSelectorOptions,
): Promise<NextStepSelection | null> {
	const enableModelCycling = models && models.length > 1 && initialModel;

	if (!enableModelCycling) {
		const p = selectScramjetChoice(ctx, {
			title: "Select next step",
			options,
			recommended,
			getTitle: optionTitle,
			getDescription: (option) => cleanDisplay(option.reason),
			autoSelect,
			countdownSeconds,
			signal,
		});
		// Avoid extra microtask tick from async — tests depend on promise depth.
		return p.then((step) => (step ? { step, model: null } : null));
	}

	const modelList = models!;
	const initialIndex = modelList.findIndex((m) => modelsAreEqual(m, initialModel!));
	// -1 sentinel: current model is off-list (auth revoked / scoped)

	if (signal?.aborted) return Promise.resolve(null);

	let fail: (err: unknown) => void = () => {};
	const failure = new Promise<never>((_resolve, reject) => {
		fail = reject;
	});

	const items = options.map((option) => ({
		value: String(option.index),
		label: `${option.index}: ${optionTitle(option)}`,
		description: cleanDisplay(option.reason),
	}));
	const byValue = new Map(options.map((option) => [String(option.index), option]));
	const recommendedIndex = recommended ? options.findIndex((o) => o.index === recommended.index) : -1;

	const selectedValue = Promise.race([
		ctx.ui.custom<{ value: string; modelIndex: number } | null>((tui, theme, _keybindings, done) => {
			let remaining = countdownSeconds;
			let timer: ReturnType<typeof setInterval> | null = null;
			let finished = false;
			let modelIndex = initialIndex;

			const selectList = new MultiLineSelectList(
				items,
				Math.min(items.length, 8),
				{
					selectedText: (text) => theme.fg("accent", text),
					description: (text) => theme.fg("muted", text),
					scrollInfo: (text) => theme.fg("dim", text),
				},
				{ recommendedIndex },
			);

			if (recommendedIndex >= 0) selectList.setSelectedIndex(recommendedIndex);

			const abort = () => finish(null);

			function clearTimer() {
				if (timer) {
					clearInterval(timer);
					timer = null;
				}
			}

			function finish(value: { value: string; modelIndex: number } | null) {
				if (finished) return;
				finished = true;
				clearTimer();
				signal?.removeEventListener("abort", abort);
				done(value);
			}

			selectList.onSelect = (item) => finish({ value: item.value, modelIndex });
			selectList.onCancel = () => finish(null);

			signal?.addEventListener("abort", abort, { once: true });
			if (signal?.aborted) finish(null);

			if (!finished && autoSelect && remaining > 0) {
				timer = setInterval(() => {
					try {
						remaining--;
						if (remaining <= 0) {
							finish({ value: String(autoSelect.index), modelIndex: initialIndex });
						} else {
							tui.requestRender();
						}
					} catch (err) {
						fail(err);
						finish(null);
					}
				}, 1000);
			}

			function modelDisplayName(idx: number): string {
				if (idx === -1) return initialModel!.name;
				return modelList[idx].name;
			}

			function modelDisplayDetail(idx: number): string {
				const m = idx === -1 ? initialModel! : modelList[idx];
				return `${m.provider}/${m.id}`;
			}

			return {
				render(width: number) {
					const modelLine = theme.fg(
						"dim",
						`model: ${modelDisplayName(modelIndex)} (${modelDisplayDetail(modelIndex)})  ←/→ change`,
					);
					const footer = timer
						? `←→ model • ↑↓ navigate • enter select • esc cancel • auto-selects recommendation in ${remaining}s`
						: "←→ model • ↑↓ navigate • enter select • esc cancel";
					return [
						theme.fg("accent", theme.bold("Select next step")),
						...selectList.render(width),
						modelLine,
						theme.fg("dim", footer),
					];
				},
				invalidate() {
					selectList.invalidate();
				},
				handleInput(data: string) {
					try {
						clearTimer();
						const kb = getKeybindings();
						if (kb.matches(data, "tui.editor.cursorLeft")) {
							const minIdx = initialIndex === -1 ? -1 : 0;
							modelIndex = modelIndex <= minIdx ? modelList.length - 1 : modelIndex - 1;
							tui.requestRender();
						} else if (kb.matches(data, "tui.editor.cursorRight")) {
							const maxIdx = modelList.length - 1;
							modelIndex = modelIndex >= maxIdx ? (initialIndex === -1 ? -1 : 0) : modelIndex + 1;
							tui.requestRender();
						} else {
							selectList.handleInput(data);
							tui.requestRender();
						}
					} catch (err) {
						fail(err);
						finish(null);
					}
				},
				dispose() {
					clearTimer();
				},
			};
		}),
		failure,
	]);

	return selectedValue.then((result) => {
		if (!result) return null;
		const step = byValue.get(result.value);
		if (!step) throw new Error(`selector returned unknown option value ${JSON.stringify(result.value)}`);
		const modelChanged = result.modelIndex !== initialIndex;
		const selectedModel = modelChanged ? (result.modelIndex === -1 ? null : modelList[result.modelIndex]) : null;
		return { step, model: selectedModel };
	});
}
