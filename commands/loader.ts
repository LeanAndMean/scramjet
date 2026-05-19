import { basename } from "node:path";
import { parseFrontmatter } from "@earendil-works/pi-coding-agent";
import type { AgentDef, AgentRegistry, CommandDef, CommandRegistry } from "../types.ts";
import { parseNextStepPolicy } from "./parse-next-step.ts";

export interface FileEntry {
	filePath: string;
	content: string;
	setName: string;
	scope: "global" | "project";
}

export type LoadResult = { ok: true; def: CommandDef } | { ok: false; error: string };

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
	return { ok: true, def };
}

export function buildRegistry(entries: FileEntry[]): RegistryBuildResult {
	const registry: CommandRegistry = new Map();
	const warnings: string[] = [];
	for (const entry of entries) {
		const result = parseCommandFile(entry.filePath, entry.content, entry.setName);
		if (!result.ok) {
			warnings.push(`skipping ${entry.filePath}: ${result.error}`);
			continue;
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
	const agentRegistry: AgentRegistry = new Map();
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
