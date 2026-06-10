import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { ValidatedNextStep } from "./commands/validator.ts";
import { MultiLineSelectList } from "./multi-line-select.ts";

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

export interface NextStepSelectorOptions {
	options: ValidatedNextStep[];
	recommended: ValidatedNextStep | null;
	autoSelect?: ValidatedNextStep;
	countdownSeconds?: number;
	signal?: AbortSignal;
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
	{ options, recommended, autoSelect, countdownSeconds = 0, signal }: NextStepSelectorOptions,
): Promise<ValidatedNextStep | null> {
	return selectScramjetChoice(ctx, {
		title: "Select next step",
		options,
		recommended,
		getTitle: optionTitle,
		getDescription: (option) => cleanDisplay(option.reason),
		autoSelect,
		countdownSeconds,
		signal,
	});
}
