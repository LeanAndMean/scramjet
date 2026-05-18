import { readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "../types.ts";
import { buildRegistry, type FileEntry } from "./loader.ts";

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

function collectEntries(rootDir: string, scope: "global" | "project"): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const setEntry of safeReaddir(rootDir)) {
		if (!setEntry.isDirectory) continue;
		const setName = setEntry.name;
		const commandsDir = join(rootDir, setName, "commands");
		for (const fileEntry of safeReaddir(commandsDir)) {
			if (fileEntry.isDirectory) continue;
			if (!fileEntry.name.endsWith(".md")) continue;
			const filePath = join(commandsDir, fileEntry.name);
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
		const globalEntries = collectEntries(globalRoot(), "global");
		const projectEntries = collectEntries(join(event.cwd, ".scramjet"), "project");
		const { registry, warnings } = buildRegistry([...globalEntries, ...projectEntries]);
		state.registry = registry;
		for (const warning of warnings) console.warn(`[scramjet] ${warning}`);
		const promptPaths: string[] = [];
		for (const def of registry.values()) promptPaths.push(def.filePath);
		return { promptPaths };
	});
}
