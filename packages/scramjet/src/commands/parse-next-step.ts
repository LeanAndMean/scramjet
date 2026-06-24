import type { Candidate, NextStepPolicy } from "../types.js";

export type ParseResult = { ok: true; policy: NextStepPolicy | null } | { ok: false; error: string };

function isObject(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseCandidates(raw: unknown): { ok: true; candidates: Candidate[] } | { ok: false; error: string } {
	if (!Array.isArray(raw)) return { ok: false, error: "candidates must be a list" };
	const candidates: Candidate[] = [];
	for (let i = 0; i < raw.length; i++) {
		const entry = raw[i];
		if (!isObject(entry)) return { ok: false, error: `candidates[${i}] must be a mapping` };
		if (typeof entry.name !== "string" || entry.name.trim() === "") {
			return { ok: false, error: `candidates[${i}].name must be a non-empty string` };
		}
		if (entry.hint !== undefined && typeof entry.hint !== "string") {
			return { ok: false, error: `candidates[${i}].hint must be a string when set` };
		}
		if (typeof entry.hint === "string" && entry.hint.trim() === "") {
			return { ok: false, error: `candidates[${i}].hint must be a non-empty string when set` };
		}
		candidates.push(entry.hint === undefined ? { name: entry.name } : { name: entry.name, hint: entry.hint });
	}
	return { ok: true, candidates };
}

function parseStringList(raw: unknown, field: string): { ok: true; values: string[] } | { ok: false; error: string } {
	if (!Array.isArray(raw)) return { ok: false, error: `${field} must be a list` };
	const values: string[] = [];
	for (let i = 0; i < raw.length; i++) {
		const entry = raw[i];
		if (typeof entry !== "string" || entry.trim() === "") {
			return { ok: false, error: `${field}[${i}] must be a non-empty string` };
		}
		values.push(entry);
	}
	return { ok: true, values };
}

export function parseNextStepPolicy(frontmatter: Record<string, unknown>): ParseResult {
	const raw = frontmatter.next;
	if (raw === undefined || raw === null) return { ok: true, policy: null };
	if (!isObject(raw)) return { ok: false, error: "next must be a mapping" };

	const mode = raw.mode;
	if (typeof mode !== "string") return { ok: false, error: "next.mode must be a string" };

	switch (mode) {
		case "forced": {
			if (typeof raw.target !== "string" || raw.target.trim() === "") {
				return { ok: false, error: "next.target must be a non-empty string for mode: forced" };
			}
			return { ok: true, policy: { mode: "forced", target: raw.target } };
		}
		case "closed": {
			const parsed = parseCandidates(raw.candidates);
			if (!parsed.ok) return { ok: false, error: `next.${parsed.error}` };
			if (parsed.candidates.length === 0) {
				return { ok: false, error: "next.candidates must not be empty for mode: closed" };
			}
			return { ok: true, policy: { mode: "closed", candidates: parsed.candidates } };
		}
		case "open": {
			const parsed = parseCandidates(raw.candidates);
			if (!parsed.ok) return { ok: false, error: `next.${parsed.error}` };
			if (raw.blacklist === undefined) {
				return { ok: true, policy: { mode: "open", candidates: parsed.candidates } };
			}
			const blacklist = parseStringList(raw.blacklist, "blacklist");
			if (!blacklist.ok) return { ok: false, error: `next.${blacklist.error}` };
			return { ok: true, policy: { mode: "open", candidates: parsed.candidates, blacklist: blacklist.values } };
		}
		case "ask": {
			if (raw.hint === undefined) return { ok: true, policy: { mode: "ask" } };
			if (typeof raw.hint !== "string") return { ok: false, error: "next.hint must be a string when set" };
			if (raw.hint.trim() === "") {
				return { ok: false, error: "next.hint must be a non-empty string when set" };
			}
			return { ok: true, policy: { mode: "ask", hint: raw.hint } };
		}
		default:
			return { ok: false, error: `unknown next.mode: ${mode}` };
	}
}
