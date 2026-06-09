import type { ExtensionContext } from "@earendil-works/pi-coding-agent";
import { type SelectItem, SelectList } from "@earendil-works/pi-tui";
import type { ValidatedNextStep } from "./commands/validator.ts";
import { buildNextStepWire } from "./next-step-dispatch.ts";

export interface NextStepSelectorOptions {
	options: ValidatedNextStep[];
	recommended: ValidatedNextStep | null;
	autoSelect?: Extract<ValidatedNextStep, { type: "command" }>;
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
	if (option.type === "command") return option.label ?? buildNextStepWire(option.step);
	return option.label ?? `Text: ${cleanDisplay(option.text)}`;
}

export async function selectNextStep(
	ctx: ExtensionContext,
	{ options, recommended, autoSelect, countdownSeconds = 0, signal }: NextStepSelectorOptions,
): Promise<ValidatedNextStep | null> {
	if (signal?.aborted) return null;

	let fail: (err: unknown) => void = () => {};
	const failure = new Promise<never>((_resolve, reject) => {
		fail = reject;
	});
	const byValue = new Map(options.map((option) => [String(option.index), option]));
	const items: SelectItem[] = options.map((option) => ({
		value: String(option.index),
		label: `${option.index}: ${optionTitle(option)}${option.index === recommended?.index ? " [recommended]" : ""}`,
		description: cleanDisplay(option.reason),
	}));

	const selectedIndex = recommended ? options.findIndex((option) => option.index === recommended.index) : -1;
	const selectedValue = await Promise.race([
		ctx.ui.custom<string | null>((tui, theme, _keybindings, done) => {
			let remaining = countdownSeconds;
			let timer: ReturnType<typeof setInterval> | null = null;
			let finished = false;

			const selectList = new SelectList(items, Math.min(items.length, 8), {
				selectedPrefix: (text) => theme.fg("accent", text),
				selectedText: (text) => theme.fg("accent", text),
				description: (text) => theme.fg("muted", text),
				scrollInfo: (text) => theme.fg("dim", text),
				noMatch: (text) => theme.fg("warning", text),
			});

			if (selectedIndex >= 0) selectList.setSelectedIndex(selectedIndex);

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
					return [
						theme.fg("accent", theme.bold("Select next step")),
						...selectList.render(width),
						theme.fg("dim", footer),
					];
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
