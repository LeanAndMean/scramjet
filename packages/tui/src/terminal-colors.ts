export interface TerminalRgb {
	r: number;
	g: number;
	b: number;
}

export const OSC_11_QUERY = "\x1b]11;?\x1b\\";

const OSC_11_PREFIX = "\x1b]11;";

// rgb:R/G/B where each channel is 1-4 hex digits
const RGB_PATTERN = /^rgb:([0-9a-fA-F]{1,4})\/([0-9a-fA-F]{1,4})\/([0-9a-fA-F]{1,4})$/;

// Hash forms: #RGB (3), #RRGGBB (6), #RRRGGGBBB (9), #RRRRGGGGBBBB (12)
const HASH_PATTERN = /^#([0-9a-fA-F]+)$/;
const VALID_HASH_LENGTHS = new Set([3, 6, 9, 12]);

function normalizeChannel(hex: string): number {
	const maxValue = (1 << (hex.length * 4)) - 1;
	return parseInt(hex, 16) / maxValue;
}

function stripTerminator(data: string): string | undefined {
	if (data.endsWith("\x07")) return data.slice(0, -1);
	if (data.endsWith("\x1b\\")) return data.slice(0, -2);
	return undefined;
}

export function parseOsc11Response(data: string): TerminalRgb | undefined {
	if (!data.startsWith(OSC_11_PREFIX)) return undefined;

	const stripped = stripTerminator(data);
	if (stripped === undefined) return undefined;
	if (stripped.length !== data.length - (data.endsWith("\x07") ? 1 : 2)) return undefined;

	const payload = stripped.slice(OSC_11_PREFIX.length);

	const rgbMatch = payload.match(RGB_PATTERN);
	if (rgbMatch) {
		return {
			r: normalizeChannel(rgbMatch[1]!),
			g: normalizeChannel(rgbMatch[2]!),
			b: normalizeChannel(rgbMatch[3]!),
		};
	}

	const hashMatch = payload.match(HASH_PATTERN);
	if (hashMatch) {
		const hex = hashMatch[1]!;
		if (!VALID_HASH_LENGTHS.has(hex.length)) return undefined;
		const chLen = hex.length / 3;
		return {
			r: normalizeChannel(hex.slice(0, chLen)),
			g: normalizeChannel(hex.slice(chLen, chLen * 2)),
			b: normalizeChannel(hex.slice(chLen * 2)),
		};
	}

	return undefined;
}

export function isOsc11Response(data: string): boolean {
	return data.startsWith(OSC_11_PREFIX);
}
