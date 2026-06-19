import { readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type { ScramjetState } from "../types.ts";
import { ensureAgentBridge } from "./agent-bridge.ts";
import { buildAgentRegistry, buildRegistry, type FileEntry } from "./loader.ts";

// Stage 9 / F6,F7,F19: filesystem-discovery error handling. These helpers
// used to swallow all errors silently; now they collect human-readable
// warnings through an out-parameter so the `resources_discover` hook can
// surface them through the Scramjet logger. ENOENT is still treated as
// "absent and fine" — only unexpected errors (EACCES, EIO, …) are reported.
function safeReaddir(dir: string, warnings: string[]): { name: string; isDirectory: boolean }[] {
	let raw: ReturnType<typeof readdirSync>;
	try {
		raw = readdirSync(dir, { withFileTypes: true });
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		if (code !== "ENOENT") {
			warnings.push(`[scramjet/discovery] could not scan ${dir} (${code ?? "unknown"}: ${(err as Error).message})`);
		}
		return [];
	}
	return raw.map((e) => {
		let isDirectory = e.isDirectory();
		if (e.isSymbolicLink()) {
			try {
				isDirectory = statSync(join(dir, e.name)).isDirectory();
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				if (code !== "ENOENT") {
					warnings.push(
						`[scramjet/discovery] could not stat symlink ${join(dir, e.name)} (${code ?? "unknown"}: ${(err as Error).message}); treating as non-directory`,
					);
				}
				isDirectory = false;
			}
		}
		return { name: e.name, isDirectory };
	});
}

function collectEntries(rootDir: string, scope: "global" | "project", subdir: string, warnings: string[]): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const setEntry of safeReaddir(rootDir, warnings)) {
		if (!setEntry.isDirectory) continue;
		const setName = setEntry.name;
		const dir = join(rootDir, setName, subdir);
		for (const fileEntry of safeReaddir(dir, warnings)) {
			if (fileEntry.isDirectory) continue;
			if (!fileEntry.name.endsWith(".md")) continue;
			const filePath = join(dir, fileEntry.name);
			let content: string;
			try {
				content = readFileSync(filePath, "utf-8");
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				warnings.push(
					`[scramjet/discovery] could not read ${filePath} (${code ?? "unknown"}: ${(err as Error).message}); skipping`,
				);
				continue;
			}
			entries.push({ filePath, content, setName, scope });
		}
	}
	return entries;
}

function globalRoot(): string {
	// Mirror scripts/postinstall.js: honor SCRAMJET_CACHE (explicit override),
	// else XDG_DATA_HOME (XDG spec), else ~/.local/share/scramjet. If the
	// seeder and the loader disagree on this path, every postinstall write
	// lands somewhere the runtime can't see. (F3)
	return (
		process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "scramjet")
	);
}

export function registerCommandLoader(pi: ExtensionAPI, state: ScramjetState): void {
	pi.on("resources_discover", (event) => {
		// F19: the hook body calls only pure functions today, but a stray
		// crash here would prevent Pi from completing resource discovery for
		// the whole session. Wrap once at the top level so any unexpected
		// failure degrades to "no scramjet commands this session" with a
		// loud warning, rather than a hard startup crash.
		try {
			const discoveryWarnings: string[] = [];
			const globalDir = globalRoot();
			const projectDir = join(event.cwd, ".scramjet");

			const globalEntries = collectEntries(globalDir, "global", "commands", discoveryWarnings);
			const projectEntries = collectEntries(projectDir, "project", "commands", discoveryWarnings);
			const { registry, warnings } = buildRegistry([...globalEntries, ...projectEntries]);
			state.registry = registry;

			const globalAgentEntries = collectEntries(globalDir, "global", "agents", discoveryWarnings);
			const projectAgentEntries = collectEntries(projectDir, "project", "agents", discoveryWarnings);
			const { agentRegistry, warnings: agentWarnings } = buildAgentRegistry([
				...globalAgentEntries,
				...projectAgentEntries,
			]);
			state.agentRegistry = agentRegistry;

			const bridge = ensureAgentBridge(agentRegistry, [globalDir, projectDir]);
			if (bridge.created.length > 0 && bridge.targetDir !== null) {
				state.logger.debug("discovery", `bridged ${bridge.created.length} agent(s) into ${bridge.targetDir}`);
			}
			if (bridge.pruned.length > 0 && bridge.targetDir !== null) {
				state.logger.debug(
					"discovery",
					`pruned ${bridge.pruned.length} stale agent symlink(s) from ${bridge.targetDir}`,
				);
			}

			for (const warning of discoveryWarnings) {
				state.logger.warn("discovery", warning);
			}
			for (const warning of [...warnings, ...agentWarnings, ...bridge.warnings]) {
				state.logger.warn("discovery", warning);
			}
			const promptPaths: string[] = [];
			for (const def of registry.values()) promptPaths.push(def.filePath);
			return { promptPaths };
		} catch (err) {
			state.logger.warn(
				"discovery",
				`failed: ${(err as Error).message}; no scramjet commands will be available this session`,
			);
			return { promptPaths: [] };
		}
	});
}
