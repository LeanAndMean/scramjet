// Agent-file transform invoked by install.sh on each bundled-plugin agent.
// Performs two frontmatter edits to make Claude-Code-authored agents Pi-
// compatible:
//   1. Strip any `model: inherit` line (Pi has no `inherit` value; other
//      model values pass through unchanged).
//   2. Rewrite `tools: [a, b, c]` inline arrays and `tools:\n  - a\n  - b`
//      block sequences to comma-string form (`tools: a, b, c`), which is
//      what Pi's tools allowlist parser expects.
//
// Unrepresentable or unsupported shapes (`tools: []`, nested arrays, flow
// maps, comments inside a block sequence, empty block sequences, complex
// item values) cause a source-path-tagged error to stderr and exit code 1.
// Failing loud is deliberate: emitting `tools:` (null) on these inputs
// would silently grant the agent every tool, which is the exact silent
// privilege escalation this transform exists to prevent.
//
// CLI contract (invoked by install.sh):
//   node src/install/transform.mjs <src> <dest>
//   exit 0 on success; exit 1 on any unrepresentable shape; messages on
//   stderr; nothing on stdout. <src> is never modified; <dest> is written
//   in full or not at all (the bash wrapper writes to a temp path).

import { readFileSync, writeFileSync } from "node:fs";
import { argv } from "node:process";
import { fileURLToPath } from "node:url";

export class TransformError extends Error {
	constructor(message) {
		super(message);
		this.name = "TransformError";
	}
}

// Returns true if a tools-array item value contains a nested array/object
// or any other shape the simple transform can't safely round-trip.
function isComplexValue(v) {
	return /[[\]{}]/.test(v);
}

// Split on commas at paren depth 0, so Claude Code's documented
// `Bash(npm:*, git:*)` allowlist syntax survives instead of being mangled
// into three malformed entries.
function splitTopLevelCommas(s) {
	const out = [];
	let cur = "";
	let depth = 0;
	for (let k = 0; k < s.length; k++) {
		const c = s[k];
		if (c === "(") depth += 1;
		else if (c === ")") depth -= 1;
		else if (c === "," && depth === 0) {
			out.push(cur);
			cur = "";
			continue;
		}
		cur += c;
	}
	if (cur.length > 0) out.push(cur);
	return out;
}

/**
 * Apply the two frontmatter rewrites to a raw agent file's contents.
 *
 * @param {string} raw - Full file contents (including any frontmatter and body).
 * @param {string} srcLabel - Path or identifier used in error messages.
 * @returns {string} Transformed file contents.
 * @throws {TransformError} On any unrepresentable or unsupported shape.
 *
 * Output is idempotent: feeding the return value back in produces a byte-
 * identical result. The body is preserved verbatim with CRLF normalized
 * to LF.
 */
export function transformAgentSource(raw, srcLabel) {
	const fmMatch = raw.match(/^---\r?\n([\s\S]*?)\r?\n---\r?\n?/);
	if (!fmMatch) {
		return raw.replace(/\r\n/g, "\n");
	}
	const fm = fmMatch[1];
	const body = raw.slice(fmMatch[0].length).replace(/\r\n/g, "\n");

	const lines = fm.split(/\r?\n/);
	const out = [];
	let i = 0;
	while (i < lines.length) {
		const line = lines[i];
		if (/^\s*model:\s*inherit\s*$/.test(line)) {
			i += 1;
			continue;
		}
		// Strict inline shape: any nested brackets or braces fall through to
		// the flow-map / nested-array branch below and fail loud.
		const inlineMatch = line.match(/^(\s*tools:\s*)\[([^[\]{}]*)\]\s*$/);
		if (inlineMatch) {
			const inner = splitTopLevelCommas(inlineMatch[2])
				.map((s) => s.trim().replace(/^["']|["']$/g, ""))
				.filter(Boolean);
			if (inner.length === 0) {
				// `tools: []` literally means "no tools allowed" in Claude Code.
				// Pi parses tools as a comma-list and treats empty as "no
				// restriction" (all tools allowed), so the semantic cannot be
				// round-tripped. Refuse rather than silently invert.
				throw new TransformError(
					`${srcLabel}: tools: [] cannot be represented in Pi (no way to express "no tools allowed"). ` +
						`Remove the tools: line or specify at least one tool.`,
				);
			}
			out.push(`${inlineMatch[1]}${inner.join(", ")}`);
			i += 1;
			continue;
		}
		// Nested array or flow-map shape that the simple transform can't
		// safely round-trip. Fail loud rather than emit malformed YAML.
		if (/^\s*tools:\s*[[{]/.test(line)) {
			throw new TransformError(`${srcLabel}: unsupported tools array shape: ${line.trim()}`);
		}
		// Block sequence: items must be strictly more indented than the
		// `tools:` key itself.
		const blockMatch = line.match(/^(\s*)tools:\s*$/);
		if (blockMatch) {
			const indent = blockMatch[1];
			const items = [];
			let j = i + 1;
			const itemRe = /^(\s+)-\s+(.*\S)\s*$/;
			while (j < lines.length) {
				const cur = lines[j];
				if (/^\s*$/.test(cur)) break;
				// A comment between `tools:` and its items, or interleaved with
				// items, makes the boundaries of the sequence ambiguous. Refuse
				// rather than silently drop items.
				if (/^\s*#/.test(cur)) {
					throw new TransformError(
						`${srcLabel}: comment line inside tools: block sequence at line ${j + 1}: ${cur.trim()}. ` +
							`Comments inside the sequence are not supported; remove the comment or rewrite as inline 'tools: a, b'.`,
					);
				}
				const m = cur.match(itemRe);
				if (!m) break;
				if (m[1].length <= indent.length) break;
				if (isComplexValue(m[2])) {
					throw new TransformError(
						`${srcLabel}: unsupported nested block-sequence item at line ${j + 1}: ${cur.trim()}`,
					);
				}
				items.push(m[2].replace(/^["']|["']$/g, ""));
				j += 1;
			}
			if (items.length === 0) {
				// `tools:` with nothing usable underneath: either the source meant
				// "no tools" (not expressible in Pi) or the parser failed to find
				// items. Either way, emitting `tools:` (null) would silently grant
				// all tools.
				throw new TransformError(
					`${srcLabel}: tools: block sequence at line ${i + 1} has no items. ` +
						`Either remove the line or list at least one item under it.`,
				);
			}
			out.push(`${indent}tools: ${items.join(", ")}`);
			i = j;
			continue;
		}
		out.push(line);
		i += 1;
	}

	return `---\n${out.join("\n")}\n---\n${body}`;
}

// CLI entry guarded so vitest imports don't try to read argv as a source path.
if (import.meta.url === `file://${argv[1]}` || fileURLToPath(import.meta.url) === argv[1]) {
	const src = argv[2];
	const dest = argv[3];
	if (!src || !dest) {
		console.error("Usage: node src/install/transform.mjs <src> <dest>");
		process.exit(2);
	}
	let raw;
	try {
		raw = readFileSync(src, "utf8");
	} catch (err) {
		console.error(`${src}: failed to read source: ${err.message}`);
		process.exit(1);
	}
	let transformed;
	try {
		transformed = transformAgentSource(raw, src);
	} catch (err) {
		if (err instanceof TransformError) {
			console.error(err.message);
			process.exit(1);
		}
		throw err;
	}
	try {
		writeFileSync(dest, transformed);
	} catch (err) {
		console.error(`${dest}: failed to write transformed output: ${err.message}`);
		process.exit(1);
	}
}
