import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export const SCRAMJET_LOG_TYPE = "scramjet:log";

export interface ScramjetLogEntry {
	level: "debug" | "warn" | "lifecycle";
	category: string;
	message: string;
	data?: Record<string, unknown>;
	timestamp: number;
}

export interface ScramjetLogger {
	warn(category: string, message: string, data?: Record<string, unknown>): void;
	debug(category: string, message: string, data?: Record<string, unknown>): void;
	lifecycle(message: string, data: Record<string, unknown>): void;
	setHasUI(value: boolean): void;
}

export function createLogger(pi: ExtensionAPI): ScramjetLogger {
	let hasUI = false;

	function append(
		level: ScramjetLogEntry["level"],
		category: string,
		message: string,
		data?: Record<string, unknown>,
	) {
		const entry: ScramjetLogEntry = { level, category, message, timestamp: Date.now() };
		if (data !== undefined) entry.data = data;
		try {
			pi.appendEntry(SCRAMJET_LOG_TYPE, entry);
		} catch {}
	}

	return {
		warn(category, message, data?) {
			append("warn", category, message, data);
			if (!hasUI) process.stderr.write(`[scramjet/${category}] ${message}\n`);
		},
		debug(category, message, data?) {
			append("debug", category, message, data);
		},
		lifecycle(message, data) {
			append("lifecycle", "lifecycle", message, data);
		},
		setHasUI(value) {
			hasUI = value;
		},
	};
}
