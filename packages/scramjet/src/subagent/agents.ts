import * as fs from "node:fs";
import * as path from "node:path";
import { getAgentDir, parseFrontmatter } from "@leanandmean/coding-agent";

export const AGENT_SCOPES = ["user", "project", "both"] as const;
export type AgentScope = (typeof AGENT_SCOPES)[number];

export interface AgentConfig {
	name: string;
	description: string;
	tools?: string[];
	model?: string;
	systemPrompt: string;
	source: "user" | "project";
	filePath: string;
}

export interface AgentDiscoveryResult {
	agents: AgentConfig[];
	projectAgentsDir: string | null;
	diagnostics: string[];
}

function errorMessage(err: unknown): string {
	return err instanceof Error ? err.message : String(err);
}

function loadAgentsFromDir(dir: string, source: "user" | "project", diagnostics: string[]): AgentConfig[] {
	const agents: AgentConfig[] = [];

	if (!fs.existsSync(dir)) {
		return agents;
	}

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
		let content: string;
		try {
			content = fs.readFileSync(filePath, "utf-8");
		} catch (err) {
			diagnostics.push(`${filePath}: failed to read agent file (${errorMessage(err)})`);
			continue;
		}

		let parsed: { frontmatter: Record<string, unknown>; body: string };
		try {
			parsed = parseFrontmatter<Record<string, unknown>>(content);
		} catch (err) {
			diagnostics.push(`${filePath}: invalid YAML frontmatter (${errorMessage(err)})`);
			continue;
		}

		const { frontmatter, body } = parsed;
		const name = typeof frontmatter.name === "string" ? frontmatter.name.trim() : "";
		const description = typeof frontmatter.description === "string" ? frontmatter.description.trim() : "";

		if (!name || !description) {
			diagnostics.push(`${filePath}: frontmatter must include string name and description`);
			continue;
		}

		const tools =
			typeof frontmatter.tools === "string"
				? frontmatter.tools
						.split(",")
						.map((t) => t.trim())
						.filter(Boolean)
				: undefined;
		if (frontmatter.tools !== undefined && typeof frontmatter.tools !== "string") {
			diagnostics.push(`${filePath}: ignoring non-string tools frontmatter`);
		}
		const model =
			typeof frontmatter.model === "string" && frontmatter.model.trim() ? frontmatter.model.trim() : undefined;
		if (frontmatter.model !== undefined && typeof frontmatter.model !== "string") {
			diagnostics.push(`${filePath}: ignoring non-string model frontmatter`);
		}

		agents.push({
			name,
			description,
			tools: tools && tools.length > 0 ? tools : undefined,
			model,
			systemPrompt: body,
			source,
			filePath,
		});
	}

	return agents;
}

function isDirectory(p: string): boolean {
	try {
		return fs.statSync(p).isDirectory();
	} catch {
		return false;
	}
}

function findNearestProjectAgentsDir(cwd: string): string | null {
	let currentDir = cwd;
	while (true) {
		const candidate = path.join(currentDir, ".scramjet", "agents");
		if (isDirectory(candidate)) return candidate;

		const parentDir = path.dirname(currentDir);
		if (parentDir === currentDir) return null;
		currentDir = parentDir;
	}
}

export function discoverAgents(cwd: string, scope: AgentScope): AgentDiscoveryResult {
	const userDir = path.join(getAgentDir(), "agents");
	const projectAgentsDir = findNearestProjectAgentsDir(cwd);
	const diagnostics: string[] = [];

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

	return { agents: Array.from(agentMap.values()), projectAgentsDir, diagnostics };
}
