/**
 * Argument parsing + placeholder substitution for command bodies.
 * Mirrors Pi's `parseCommandArgs` and `substituteArgs` from
 * `core/prompt-templates.js` so a scramjet-expanded command body looks
 * identical to a Pi-expanded `/slash arg1 arg2` invocation. Used by
 * `delegate.ts` (subroutine bodies) and `auto-continue.ts` (next-step
 * dispatch bodies); kept in `commands/` because the substitution
 * semantics belong to the command-loader surface, not to any one
 * caller.
 */

// Mirrors Pi's parseCommandArgs (core/prompt-templates.js): bash-style
// whitespace split with single/double-quote grouping. Replicated so the
// expanded body matches what a /slash invocation would produce.
export function parseDelegateArgs(argsString: string): string[] {
	const args: string[] = [];
	let current = "";
	let inQuote: string | null = null;
	for (let i = 0; i < argsString.length; i++) {
		const char = argsString[i];
		if (inQuote) {
			if (char === inQuote) inQuote = null;
			else current += char;
		} else if (char === '"' || char === "'") {
			inQuote = char;
		} else if (char === " " || char === "\t") {
			if (current) {
				args.push(current);
				current = "";
			}
		} else {
			current += char;
		}
	}
	if (current) args.push(current);
	return args;
}

// Mirrors Pi's substituteArgs. Positional ($1, $2, ...) substituted BEFORE
// wildcards ($@, $ARGUMENTS, ${@:N:L}) so a value containing $<digit>
// doesn't re-substitute. Replacement happens on the template only; values
// are not recursively expanded.
export function substituteArguments(content: string, args: string[]): string {
	let result = content;
	result = result.replace(/\$(\d+)/g, (_, num) => {
		const idx = parseInt(num, 10) - 1;
		return args[idx] ?? "";
	});
	result = result.replace(/\$\{@:(\d+)(?::(\d+))?\}/g, (_, startStr, lengthStr) => {
		let start = parseInt(startStr, 10) - 1;
		if (start < 0) start = 0;
		if (lengthStr) {
			const length = parseInt(lengthStr, 10);
			return args.slice(start, start + length).join(" ");
		}
		return args.slice(start).join(" ");
	});
	const allArgs = args.join(" ");
	result = result.replace(/\$ARGUMENTS/g, allArgs);
	result = result.replace(/\$@/g, allArgs);
	return result;
}
