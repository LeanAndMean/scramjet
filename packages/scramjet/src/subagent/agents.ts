import * as fs from "node:fs";
import * as path from "node:path";
import { getAgentDir, parseFrontmatter } from "@leanandmean/coding-agent";
import type { AgentDef, AgentRegistry, AgentSource } from "../types.js";

export const AGENT_SCOPES = ["user", "project", "both"] as const;
export type AgentScope = (typeof AGENT_SCOPES)[number];
export type LooseAgentSource = "user" | "project";
export type ExecutableAgentSource = AgentSource | LooseAgentSource;

export interface AgentConfig {
	name: string;
	description: string;
	tools?: string[];
	model?: string;
	systemPrompt: string;
	source: ExecutableAgentSource;
	filePath: string;
	diagnostics?: string[];
}

export interface ParsedAgent {
	name: string;
	description: string;
	tools?: string[];
	model?: string;
	systemPrompt: string;
}

export type AgentParseResult =
	| { ok: true; agent: ParsedAgent; diagnostics: string[] }
	| { ok: false; error: string; diagnostics: string[] };

export interface AgentDiscoveryResult {
	agents: AgentConfig[];
	projectAgentsDir: string | null;
	diagnostics: string[];
}

const RESERVED_PREFIXES = ["mach12:", "scramjet:"] as const;

function errorMessage(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

function hasReservedIdentity(name: string): boolean {
	return RESERVED_PREFIXES.some((prefix) => name.startsWith(prefix));
}

export function parseExecutableAgent(filePath: string, content: string): AgentParseResult {
	const normalized = content.charCodeAt(0) === 0xfeff ? content.slice(1) : content;
	let parsed: { frontmatter: Record<string, unknown>; body: string };
	try {
		parsed = parseFrontmatter<Record<string, unknown>>(normalized);
	} catch (err) {
		return { ok: false, error: `${filePath}: invalid YAML frontmatter (${errorMessage(err)})`, diagnostics: [] };
	}

	const name = typeof parsed.frontmatter.name === "string" ? parsed.frontmatter.name.trim() : "";
	const description = typeof parsed.frontmatter.description === "string" ? parsed.frontmatter.description.trim() : "";
	if (!name || !description) {
		return {
			ok: false,
			error: `${filePath}: frontmatter must include string name and description`,
			diagnostics: [],
		};
	}

	const diagnostics: string[] = [];
	const tools =
		typeof parsed.frontmatter.tools === "string"
			? parsed.frontmatter.tools
					.split(",")
					.map((tool) => tool.trim())
					.filter(Boolean)
			: undefined;
	if (parsed.frontmatter.tools !== undefined && typeof parsed.frontmatter.tools !== "string") {
		diagnostics.push(`${filePath}: ignoring non-string tools frontmatter`);
	}
	const model =
		typeof parsed.frontmatter.model === "string" && parsed.frontmatter.model.trim()
			? parsed.frontmatter.model.trim()
			: undefined;
	if (parsed.frontmatter.model !== undefined && typeof parsed.frontmatter.model !== "string") {
		diagnostics.push(`${filePath}: ignoring non-string model frontmatter`);
	}

	const agent: ParsedAgent = { name, description, systemPrompt: parsed.body };
	if (tools && tools.length > 0) agent.tools = tools;
	if (model) agent.model = model;
	return { ok: true, agent, diagnostics };
}

function loadAgentsFromDir(dir: string, source: LooseAgentSource, diagnostics: string[]): AgentConfig[] {
	const agents: AgentConfig[] = [];

	if (!fs.existsSync(dir)) return agents;

	let entries: fs.Dirent[];
	try {
		entries = fs.readdirSync(dir, { withFileTypes: true });
	} catch (err) {
		diagnostics.push(`${dir}: failed to read agent directory (${errorMessage(err)})`);
		return agents;
	}

	for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
		if (!entry.name.endsWith(".md")) continue;
		if (!entry.isFile() && !entry.isSymbolicLink()) continue;

		const filePath = path.join(dir, entry.name);
		const fileName = entry.name.slice(0, -".md".length);
		if (hasReservedIdentity(fileName)) {
			diagnostics.push(`${filePath}: reserved agent filename cannot be loaded from loose agent directories`);
			continue;
		}

		let content: string;
		try {
			content = fs.readFileSync(filePath, "utf-8");
		} catch (err) {
			diagnostics.push(`${filePath}: failed to read agent file (${errorMessage(err)})`);
			continue;
		}

		const parsed = parseExecutableAgent(filePath, content);
		if (!parsed.ok) {
			diagnostics.push(parsed.error);
			continue;
		}
		diagnostics.push(...parsed.diagnostics);
		if (hasReservedIdentity(parsed.agent.name)) {
			diagnostics.push(`${filePath}: reserved agent identity cannot be loaded from loose agent directories`);
			continue;
		}

		agents.push({ ...parsed.agent, source, filePath });
	}

	return agents;
}

function findNearestProjectAgentsDir(cwd: string, diagnostics: string[]): string | null {
	let currentDir = cwd;
	while (true) {
		const candidate = path.join(currentDir, ".scramjet", "agents");
		try {
			if (fs.statSync(candidate).isDirectory()) return candidate;
		} catch (err) {
			const code = (err as NodeJS.ErrnoException).code;
			if (code !== "ENOENT" && code !== "ENOTDIR") {
				diagnostics.push(`${candidate}: cannot check directory (${errorMessage(err)})`);
			}
		}

		const parentDir = path.dirname(currentDir);
		if (parentDir === currentDir) return null;
		currentDir = parentDir;
	}
}

function loadRegisteredAgent(def: AgentDef, diagnostics: string[]): AgentConfig | undefined {
	let content: string;
	try {
		content = fs.readFileSync(def.filePath, "utf-8");
	} catch (err) {
		diagnostics.push(`${def.filePath}: failed to read registered agent (${errorMessage(err)})`);
		return undefined;
	}
	const parsed = parseExecutableAgent(def.filePath, content);
	if (!parsed.ok) {
		diagnostics.push(`${def.filePath}: registered agent is invalid (${parsed.error})`);
		return undefined;
	}
	const invocationDiagnostics = [...parsed.diagnostics];
	diagnostics.push(...invocationDiagnostics);
	const expectedPrefix = `${def.setName}:`;
	const fileName = path.basename(def.filePath, ".md");
	if (
		parsed.agent.name !== def.name ||
		!fileName.startsWith(expectedPrefix) ||
		!parsed.agent.name.startsWith(expectedPrefix)
	) {
		diagnostics.push(`${def.filePath}: registered identity changed from ${def.name}`);
		return undefined;
	}
	return {
		...parsed.agent,
		source: def.source,
		filePath: def.filePath,
		...(invocationDiagnostics.length > 0 ? { diagnostics: invocationDiagnostics } : {}),
	};
}

export function discoverAgents(cwd: string, scope: AgentScope, registeredAgents?: AgentRegistry): AgentDiscoveryResult {
	const userDir = path.join(getAgentDir(), "agents");
	const diagnostics: string[] = [];
	const projectAgentsDir = findNearestProjectAgentsDir(cwd, diagnostics);

	const userAgents = scope === "project" ? [] : loadAgentsFromDir(userDir, "user", diagnostics);
	const projectAgents =
		scope === "user" || !projectAgentsDir ? [] : loadAgentsFromDir(projectAgentsDir, "project", diagnostics);
	const agentMap = new Map<string, AgentConfig>();

	if (scope === "both") {
		for (const agent of userAgents) agentMap.set(agent.name, agent);
		for (const agent of projectAgents) agentMap.set(agent.name, agent);
	} else if (scope === "user") {
		for (const agent of userAgents) agentMap.set(agent.name, agent);
	} else {
		for (const agent of projectAgents) agentMap.set(agent.name, agent);
	}

	for (const def of registeredAgents?.values() ?? []) {
		agentMap.delete(def.name);
		const agent = loadRegisteredAgent(def, diagnostics);
		if (agent) agentMap.set(agent.name, agent);
	}

	return { agents: Array.from(agentMap.values()), projectAgentsDir, diagnostics };
}
