import { readdirSync, readFileSync, statSync } from "node:fs";
import { basename, dirname, extname, join, resolve } from "node:path";
import { type CommandLintDiagnostic, lintCommandEntries } from "./command-lint.js";
import type { FileEntry } from "./commands/loader.js";

const HELP = `Usage: scramjet-command-lint [--strict] [--json] <path>...
       scramjet-command-lint --help

Check Scramjet command files without loading the interactive harness.

Options:
  --strict  Exit with status 1 when warnings are present
  --json    Emit one JSON report
  --help    Show this help
`;

interface ParsedArguments {
	strict: boolean;
	json: boolean;
	targets: string[];
}

interface CommandLintReport {
	checkedFiles: string[];
	diagnostics: CommandLintDiagnostic[];
	summary: {
		files: number;
		errors: number;
		warnings: number;
	};
}

function parseArguments(args: string[]): ParsedArguments | string {
	const parsed: ParsedArguments = { strict: false, json: false, targets: [] };
	for (const arg of args) {
		if (arg === "--strict") parsed.strict = true;
		else if (arg === "--json") parsed.json = true;
		else if (arg.startsWith("-")) return `unknown option: ${arg}`;
		else parsed.targets.push(arg);
	}
	if (parsed.targets.length === 0) return "at least one path is required";
	return parsed;
}

function commandFileEntry(filePath: string, setName: string): FileEntry {
	return {
		filePath,
		content: readFileSync(filePath, "utf8"),
		setName,
		scope: "global",
	};
}

function entriesFromCommandsDirectory(commandsDir: string, setName: string): FileEntry[] {
	return readdirSync(commandsDir, { withFileTypes: true })
		.map((entry) => entry.name)
		.filter((name) => extname(name) === ".md")
		.sort()
		.map((name) => commandFileEntry(join(commandsDir, name), setName));
}

function entriesFromTarget(target: string): FileEntry[] {
	const targetPath = resolve(target);
	const stats = statSync(targetPath);
	if (stats.isFile()) {
		const fileName = basename(targetPath);
		const separator = fileName.indexOf(":");
		if (extname(fileName) !== ".md" || separator <= 0 || separator === fileName.length - 4) {
			throw new Error("command file name must have a qualified <set>:<command>.md identity");
		}
		return [commandFileEntry(targetPath, fileName.slice(0, separator))];
	}
	if (!stats.isDirectory()) throw new Error("target is not a file or directory");

	if (basename(targetPath) === "commands") {
		const setName = basename(dirname(targetPath));
		if (!setName) throw new Error("commands directory has no parent set identity");
		return entriesFromCommandsDirectory(targetPath, setName);
	}

	const commandsDir = join(targetPath, "commands");
	if (!statSync(commandsDir).isDirectory()) throw new Error("command-set root does not contain a commands directory");
	const setName = basename(targetPath);
	if (!setName) throw new Error("command-set root has no set identity");
	return entriesFromCommandsDirectory(commandsDir, setName);
}

function collectEntries(targets: string[]): FileEntry[] {
	return targets.flatMap((target) => {
		try {
			return entriesFromTarget(target);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			throw new Error(`${target}: ${message}`);
		}
	});
}

function reportFor(entries: FileEntry[]): CommandLintReport {
	const { diagnostics } = lintCommandEntries(entries);
	return {
		checkedFiles: entries.map((entry) => entry.filePath),
		diagnostics,
		summary: {
			files: entries.length,
			errors: diagnostics.filter((diagnostic) => diagnostic.severity === "error").length,
			warnings: diagnostics.filter((diagnostic) => diagnostic.severity === "warning").length,
		},
	};
}

function unicodeEscape(character: string): string {
	return `\\u${character.charCodeAt(0).toString(16).padStart(4, "0")}`;
}

function terminalSafe(value: string): string {
	return value.replace(/[\u0000-\u001f\u007f-\u009f]/g, unicodeEscape);
}

function renderJson(report: CommandLintReport): string {
	return JSON.stringify(report, null, 2).replace(/[\u007f-\u009f]/g, unicodeEscape);
}

function renderHuman(report: CommandLintReport): string {
	const diagnostics = report.diagnostics.map(
		(diagnostic) =>
			`${terminalSafe(diagnostic.filePath)}:${diagnostic.line ?? 1}: ${diagnostic.severity} ${diagnostic.code}: ${terminalSafe(diagnostic.message)}`,
	);
	const summary = `Checked ${report.summary.files} file${report.summary.files === 1 ? "" : "s"}: ${report.summary.errors} error${report.summary.errors === 1 ? "" : "s"}, ${report.summary.warnings} warning${report.summary.warnings === 1 ? "" : "s"}.`;
	return [...diagnostics, summary].join("\n");
}

export function runCommandLint(args: string[]): number {
	if (args.length === 1 && args[0] === "--help") {
		process.stdout.write(HELP);
		return 0;
	}

	const parsed = parseArguments(args);
	if (typeof parsed === "string") {
		process.stderr.write(`scramjet-command-lint: ${terminalSafe(parsed)}\n${HELP}`);
		return 2;
	}

	let entries: FileEntry[];
	try {
		entries = collectEntries(parsed.targets);
	} catch (error) {
		const message = error instanceof Error ? error.message : String(error);
		process.stderr.write(`scramjet-command-lint: ${terminalSafe(message)}\n`);
		return 2;
	}

	const report = reportFor(entries);
	process.stdout.write(parsed.json ? `${renderJson(report)}\n` : `${renderHuman(report)}\n`);
	if (report.summary.errors > 0) return 1;
	if (parsed.strict && report.summary.warnings > 0) return 1;
	return 0;
}
