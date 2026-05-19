import { readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "../types.ts";
import { ensureAgentBridge } from "./agent-bridge.ts";
import { buildAgentRegistry, buildRegistry, type FileEntry } from "./loader.ts";

function safeReaddir(dir: string): { name: string; isDirectory: boolean }[] {
	try {
		return readdirSync(dir, { withFileTypes: true }).map((e) => {
			let isDirectory = e.isDirectory();
			if (e.isSymbolicLink()) {
				try {
					isDirectory = statSync(join(dir, e.name)).isDirectory();
				} catch {
					isDirectory = false;
				}
			}
			return { name: e.name, isDirectory };
		});
	} catch {
		return [];
	}
}

function collectEntries(rootDir: string, scope: "global" | "project", subdir: string): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const setEntry of safeReaddir(rootDir)) {
		if (!setEntry.isDirectory) continue;
		const setName = setEntry.name;
		const dir = join(rootDir, setName, subdir);
		for (const fileEntry of safeReaddir(dir)) {
			if (fileEntry.isDirectory) continue;
			if (!fileEntry.name.endsWith(".md")) continue;
			const filePath = join(dir, fileEntry.name);
			let content: string;
			try {
				content = readFileSync(filePath, "utf-8");
			} catch {
				continue;
			}
			entries.push({ filePath, content, setName, scope });
		}
	}
	return entries;
}

function globalRoot(): string {
	return process.env.SCRAMJET_CACHE ?? join(homedir(), ".local", "share", "scramjet");
}

export function registerCommandLoader(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("resources_discover", (event) => {
		const globalDir = globalRoot();
		const projectDir = join(event.cwd, ".scramjet");

		const globalEntries = collectEntries(globalDir, "global", "commands");
		const projectEntries = collectEntries(projectDir, "project", "commands");
		const { registry, warnings } = buildRegistry([...globalEntries, ...projectEntries]);
		state.registry = registry;

		const globalAgentEntries = collectEntries(globalDir, "global", "agents");
		const projectAgentEntries = collectEntries(projectDir, "project", "agents");
		const { agentRegistry, warnings: agentWarnings } = buildAgentRegistry([
			...globalAgentEntries,
			...projectAgentEntries,
		]);
		state.agentRegistry = agentRegistry;

		const bridge = ensureAgentBridge(agentRegistry, [globalDir, projectDir]);
		if (bridge.created.length > 0 && bridge.targetDir !== null) {
			console.log(`[scramjet] bridged ${bridge.created.length} agent(s) into ${bridge.targetDir}`);
		}
		if (bridge.pruned.length > 0 && bridge.targetDir !== null) {
			console.log(`[scramjet] pruned ${bridge.pruned.length} stale agent symlink(s) from ${bridge.targetDir}`);
		}

		for (const warning of [...warnings, ...agentWarnings, ...bridge.warnings]) {
			console.warn(`[scramjet] ${warning}`);
		}
		const promptPaths: string[] = [];
		for (const def of registry.values()) promptPaths.push(def.filePath);
		return { promptPaths };
	});
}
