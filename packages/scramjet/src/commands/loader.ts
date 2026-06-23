import { basename } from "node:path";
import { parseFrontmatter } from "@scramjet/coding-agent";
import type { AgentDef, AgentRegistry, CommandDef, CommandRegistry } from "../types.js";
import { parseNextStepPolicy } from "./parse-next-step.js";

// Source of a discovered command/agent file. Currently used only to
// disambiguate same-name warnings; reserved for future precedence rules
// (project entries shadowing global entries with the same name).
export type FileScope = "global" | "project";

export interface FileEntry {
	filePath: string;
	content: string;
	setName: string;
	scope: FileScope;
}

export type LoadResult = { ok: true; def: CommandDef; warnings?: string[] } | { ok: false; error: string };

interface RegistryBuildResult {
	registry: CommandRegistry;
	warnings: string[];
}

export function parseAllowedTools(raw: unknown): string[] | undefined {
	if (raw === undefined || raw === null) return undefined;
	if (Array.isArray(raw)) {
		const tools = raw
			.filter((x): x is string => typeof x === "string")
			.map((s) => s.trim())
			.filter(Boolean);
		return tools.length > 0 ? tools : undefined;
	}
	if (typeof raw === "string") {
		const tools = raw
			.split(",")
			.map((s) => s.trim())
			.filter(Boolean);
		return tools.length > 0 ? tools : undefined;
	}
	return undefined;
}

export function parseCommandFile(filePath: string, content: string, setName: string): LoadResult {
	const fileName = basename(filePath);
	if (!fileName.endsWith(".md")) {
		return { ok: false, error: `${fileName}: not a markdown file` };
	}
	const name = fileName.slice(0, -".md".length);
	const expectedPrefix = `${setName}:`;
	if (!name.startsWith(expectedPrefix)) {
		return { ok: false, error: `${fileName}: filename must start with "${expectedPrefix}"` };
	}
	const normalized = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
	let parsed: { frontmatter: Record<string, unknown>; body: string };
	try {
		parsed = parseFrontmatter(normalized);
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		return { ok: false, error: `${fileName}: malformed frontmatter — ${message}` };
	}
	const nextResult = parseNextStepPolicy(parsed.frontmatter);
	if (!nextResult.ok) {
		return { ok: false, error: `${fileName}: invalid next block — ${nextResult.error}` };
	}
	const def: CommandDef = { name, filePath, body: parsed.body };
	const description = parsed.frontmatter.description;
	if (typeof description === "string" && description.trim() !== "") def.description = description;
	const allowedTools = parseAllowedTools(parsed.frontmatter["allowed-tools"]);
	if (allowedTools !== undefined) def.allowedTools = allowedTools;
	if (nextResult.policy !== null) def.next = nextResult.policy;
	// S8: parseAllowedTools silently drops non-string array entries. Detect
	// them here and surface a load warning so a typo'd YAML list (e.g. an
	// unquoted `42`, a stray `null`) is visible at startup rather than
	// shrinking the caller's tool scope without explanation.
	const toolWarnings: string[] = [];
	const rawTools = parsed.frontmatter["allowed-tools"];
	if (Array.isArray(rawTools)) {
		const nonStrings = rawTools.filter((x) => typeof x !== "string");
		if (nonStrings.length > 0) {
			toolWarnings.push(
				`${fileName}: "allowed-tools" contains ${nonStrings.length} non-string entr${nonStrings.length === 1 ? "y" : "ies"} (ignored: ${JSON.stringify(nonStrings)})`,
			);
		}
	}
	const result: LoadResult = { ok: true, def };
	if (toolWarnings.length > 0) result.warnings = toolWarnings;
	return result;
}

export function buildRegistry(entries: FileEntry[]): RegistryBuildResult {
	const registry = new Map<string, CommandDef>();
	const warnings: string[] = [];
	for (const entry of entries) {
		const result = parseCommandFile(entry.filePath, entry.content, entry.setName);
		if (!result.ok) {
			warnings.push(`skipping ${entry.filePath}: ${result.error}`);
			continue;
		}
		if (result.warnings) {
			for (const w of result.warnings) warnings.push(w);
		}
		const existing = registry.get(result.def.name);
		if (existing) {
			warnings.push(
				`skipping ${entry.scope} command ${result.def.name} at ${entry.filePath}: name already registered from ${existing.filePath}`,
			);
			continue;
		}
		registry.set(result.def.name, result.def);
	}
	return { registry, warnings };
}

export type AgentFileEntry = FileEntry;

export type AgentLoadResult = { ok: true; def: AgentDef } | { ok: false; error: string };

interface AgentRegistryBuildResult {
	agentRegistry: AgentRegistry;
	warnings: string[];
}

export function parseAgentFile(filePath: string, content: string, setName: string): AgentLoadResult {
	const fileName = basename(filePath);
	if (!fileName.endsWith(".md")) {
		return { ok: false, error: `${fileName}: not a markdown file` };
	}
	const expectedPrefix = `${setName}:`;
	if (!fileName.slice(0, -".md".length).startsWith(expectedPrefix)) {
		return { ok: false, error: `${fileName}: filename must start with "${expectedPrefix}"` };
	}
	const normalized = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
	let parsed: { frontmatter: Record<string, unknown>; body: string };
	try {
		parsed = parseFrontmatter(normalized);
	} catch (err) {
		const message = err instanceof Error ? err.message : String(err);
		return { ok: false, error: `${fileName}: malformed frontmatter — ${message}` };
	}
	const name = parsed.frontmatter.name;
	if (typeof name !== "string" || name.trim() === "") {
		return { ok: false, error: `${fileName}: missing required "name" field in frontmatter` };
	}
	const def: AgentDef = { name: name.trim(), filePath };
	const description = parsed.frontmatter.description;
	if (typeof description === "string" && description.trim() !== "") def.description = description.trim();
	return { ok: true, def };
}

export function buildAgentRegistry(entries: AgentFileEntry[]): AgentRegistryBuildResult {
	const agentRegistry = new Map<string, AgentDef>();
	const warnings: string[] = [];
	for (const entry of entries) {
		const result = parseAgentFile(entry.filePath, entry.content, entry.setName);
		if (!result.ok) {
			warnings.push(`skipping agent ${entry.filePath}: ${result.error}`);
			continue;
		}
		const existing = agentRegistry.get(result.def.name);
		if (existing) {
			warnings.push(
				`skipping ${entry.scope} agent ${result.def.name} at ${entry.filePath}: name already registered from ${existing.filePath}`,
			);
			continue;
		}
		agentRegistry.set(result.def.name, result.def);
	}
	return { agentRegistry, warnings };
}
