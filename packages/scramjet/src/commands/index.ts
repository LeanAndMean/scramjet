import { createHash } from "node:crypto";
import { readdirSync, readFileSync, type Stats, statSync } from "node:fs";
import { homedir } from "node:os";
import { basename, join, relative, resolve, sep } from "node:path";
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { parseAutonomyRecommendations, validateRecommendations } from "../autonomy-settings.js";
import { packageRoot } from "../docs-registry.js";
import type {
	AutonomyRecommendations,
	CommandRegistry,
	PublicationDefault,
	PublicationTool,
	ScramjetState,
} from "../types.js";
import {
	formatLegacyBundleWarning,
	inspectLegacyBundle,
	type LegacyBundleInspectionOptions,
} from "./legacy-bundle-inspection.js";
import { buildAgentRegistry, buildRegistry, type FileEntry } from "./loader.js";

type Scope = "global" | "project";
type SelectedSet = { name: string; dir: string; scope: Scope; source: "package" | "global" | "project" };
type Inspection = {
	stat(path: string): Stats;
	readdir(path: string): string[];
	readFile(path: string): string;
};
type PackagedSet = { set: SelectedSet; commandEntries: FileEntry[] };

const BUNDLED_SETS = ["mach12", "scramjet"] as const;

function filterPublicationDefaults(
	recommendations: AutonomyRecommendations,
	registry: CommandRegistry,
	setRoot: string,
	warnings: string[],
): AutonomyRecommendations["publications"] {
	const filtered: NonNullable<AutonomyRecommendations["publications"]> = {};
	for (const [command, settings] of Object.entries(recommendations.publications ?? {})) {
		const definition = registry.get(command);
		const commandPath = definition ? resolve(definition.filePath) : "";
		const ownerPath = resolve(setRoot);
		const pathFromOwner = definition ? relative(ownerPath, commandPath) : "..";
		const owned = pathFromOwner === "" || (!pathFromOwner.startsWith(`..${sep}`) && pathFromOwner !== "..");
		if (!definition || definition.delegateOnly || !owned) {
			warnings.push(
				`[scramjet/discovery] ignored publication defaults for non-owned top-level command "${command}"`,
			);
			continue;
		}
		for (const [tool, setting] of Object.entries(settings) as [PublicationTool, PublicationDefault][]) {
			if (!definition.allowedTools?.includes(tool)) {
				warnings.push(`[scramjet/discovery] ignored publication default for undeclared tool ${command} → ${tool}`);
				continue;
			}
			filtered[command] ??= {};
			filtered[command][tool] = setting;
		}
	}
	return Object.keys(filtered).length > 0 ? filtered : undefined;
}

function safeReaddir(
	dir: string,
	warnings: string[],
	excludedNames: readonly string[] = [],
): { name: string; isDirectory: boolean }[] {
	let raw: import("node:fs").Dirent[];
	try {
		raw = readdirSync(dir, { withFileTypes: true }) as import("node:fs").Dirent[];
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		if (code !== "ENOENT") {
			warnings.push(`[scramjet/discovery] could not scan ${dir} (${code ?? "unknown"}: ${(err as Error).message})`);
		}
		return [];
	}
	return raw
		.filter((entry) => !excludedNames.includes(String(entry.name)))
		.map((e) => {
			const name = String(e.name);
			let isDirectory = e.isDirectory();
			if (e.isSymbolicLink()) {
				try {
					isDirectory = statSync(join(dir, name)).isDirectory();
				} catch (err) {
					const code = (err as NodeJS.ErrnoException).code;
					if (code === "ENOENT") {
						warnings.push(
							`[scramjet/discovery] symlink ${join(dir, name)} has a missing target; if you migrated from the single-repo layout, remove the old symlink and re-run: ln -sfn "$(pwd)/packages/scramjet/mach12" "${join(dir, name)}"`,
						);
					} else {
						warnings.push(
							`[scramjet/discovery] could not stat symlink ${join(dir, name)} (${code ?? "unknown"}: ${(err as Error).message}); treating as non-directory`,
						);
					}
					isDirectory = false;
				}
			}
			return { name, isDirectory };
		});
}

function enumerateSets(root: string, scope: Scope, source: SelectedSet["source"], warnings: string[]): SelectedSet[] {
	return safeReaddir(root, warnings, BUNDLED_SETS)
		.filter((entry) => entry.isDirectory)
		.map((entry) => ({ name: entry.name, dir: join(root, entry.name), scope, source }));
}

function collectEntries(
	sets: SelectedSet[],
	subdir: string,
	warnings: string[],
	readFile: (path: string) => string = (path) => readFileSync(path, "utf-8"),
): FileEntry[] {
	const entries: FileEntry[] = [];
	for (const set of sets) {
		const dir = join(set.dir, subdir);
		for (const fileEntry of safeReaddir(dir, warnings)) {
			if (fileEntry.isDirectory || !fileEntry.name.endsWith(".md")) continue;
			const filePath = join(dir, fileEntry.name);
			try {
				entries.push({
					filePath,
					content: readFile(filePath),
					setName: set.name,
					scope: set.scope,
					source: set.source,
				});
			} catch (err) {
				const code = (err as NodeJS.ErrnoException).code;
				warnings.push(
					`[scramjet/discovery] could not read ${filePath} (${code ?? "unknown"}: ${(err as Error).message}); skipping`,
				);
			}
		}
	}
	return entries;
}

function packagedSet(
	name: (typeof BUNDLED_SETS)[number],
	bundledRoot: string,
	inspection: Inspection,
	warnings: string[],
): PackagedSet | undefined {
	const source = join(bundledRoot, name);
	try {
		if (!inspection.stat(source).isDirectory()) {
			warnings.push(
				`[scramjet/discovery] bundled package command set ${source} is not a directory; reinstall Scramjet`,
			);
			return undefined;
		}
		if (inspection.readdir(source).length === 0) {
			warnings.push(`[scramjet/discovery] bundled package command set ${source} is empty; reinstall Scramjet`);
			return undefined;
		}
		const commandsDir = join(source, "commands");
		if (!inspection.stat(commandsDir).isDirectory()) {
			warnings.push(
				`[scramjet/discovery] bundled package command set commands path ${commandsDir} is not a directory; reinstall Scramjet`,
			);
			return undefined;
		}
		inspection.readdir(commandsDir);
	} catch (err) {
		const code = (err as NodeJS.ErrnoException).code;
		warnings.push(
			`[scramjet/discovery] could not use bundled package command set ${source} (${code ?? "unknown"}: ${(err as Error).message}); reinstall Scramjet`,
		);
		return undefined;
	}
	const set: SelectedSet = { name, dir: source, scope: "global", source: "package" };
	const commandEntries = collectEntries([set], "commands", warnings, inspection.readFile);
	const validation = buildRegistry(commandEntries);
	if (validation.registry.size === 0) {
		warnings.push(...validation.warnings);
		warnings.push(
			`[scramjet/discovery] bundled package command set commands path ${join(source, "commands")} contains no usable commands; reinstall Scramjet`,
		);
		return undefined;
	}
	return { set, commandEntries };
}

export function commandFingerprint(content: string): string {
	return createHash("sha256").update(content).digest("hex").slice(0, 12);
}

export function normalizedCommandSource(scope: Scope, setName: string, filePath: string): string {
	const relative = `${setName}/commands/${basename(filePath)}`;
	return scope === "project" ? `.scramjet/${relative}` : relative;
}

function globalRoot(): string {
	return (
		process.env.SCRAMJET_CACHE ?? join(process.env.XDG_DATA_HOME ?? join(homedir(), ".local", "share"), "scramjet")
	);
}

export function registerCommandLoader(
	pi: ExtensionAPI,
	state: ScramjetState,
	dependencies: {
		bundledRoot?: string;
		inspection?: Partial<Inspection>;
		interactiveOutput?: boolean;
		legacyInspector?: (options: LegacyBundleInspectionOptions) => ReturnType<typeof inspectLegacyBundle>;
	} = {},
): void {
	const bundledRoot = dependencies.bundledRoot ?? packageRoot();
	const interactiveOutput = dependencies.interactiveOutput ?? Boolean(process.stdout.isTTY);
	const inspection: Inspection = {
		stat: dependencies.inspection?.stat ?? statSync,
		readdir: dependencies.inspection?.readdir ?? readdirSync,
		readFile: dependencies.inspection?.readFile ?? ((path) => readFileSync(path, "utf-8")),
	};
	const legacyInspector = dependencies.legacyInspector ?? inspectLegacyBundle;
	const journaledLegacySignatures = new Set<string>();
	const displayedLegacySignatures = new Set<string>();
	let lastPublicationWarningSignature = "";

	pi.on("resources_discover", (event, ctx) => {
		const skillPaths = [join(bundledRoot, "skills")];
		const themePaths = [join(packageRoot(), "themes")];
		try {
			const discoveryWarnings: string[] = [];
			const globalDir = globalRoot();
			const projectDir = join(event.cwd, ".scramjet");
			const packagedSources = new Map(
				BUNDLED_SETS.map((name) => [name, packagedSet(name, bundledRoot, inspection, discoveryWarnings)]),
			);
			const packagedSets = new Map(BUNDLED_SETS.map((name) => [name, packagedSources.get(name)?.set]));
			const customSets = [
				...enumerateSets(globalDir, "global", "global", discoveryWarnings),
				...enumerateSets(projectDir, "project", "project", discoveryWarnings),
			];
			const selectedSets = [
				...[...packagedSets.values()].filter((set): set is SelectedSet => set !== undefined),
				...customSets,
			];

			const commandEntries = [
				...BUNDLED_SETS.flatMap((name) => packagedSources.get(name)?.commandEntries ?? []),
				...collectEntries(customSets, "commands", discoveryWarnings),
			];
			const { registry, warnings } = buildRegistry(commandEntries);
			state.registry = registry;
			const entriesByPath = new Map(commandEntries.map((entry) => [entry.filePath, entry]));
			for (const def of registry.values()) {
				const entry = entriesByPath.get(def.filePath);
				if (!entry) continue;
				state.logger.debug("discovery", "command discovered", {
					command: def.name,
					scope: entry.scope,
					source: normalizedCommandSource(entry.scope, entry.setName, entry.filePath),
					fingerprint: commandFingerprint(entry.content),
				});
			}

			const { agentRegistry, warnings: agentWarnings } = buildAgentRegistry(
				collectEntries(selectedSets, "agents", discoveryWarnings),
			);
			state.agentRegistry = agentRegistry;

			const recommendations = new Map<string, import("../types.js").AutonomyRecommendations>();
			for (const set of selectedSets) {
				const recPath = join(set.dir, "autonomy-defaults.yaml");
				let content: string;
				try {
					content = readFileSync(recPath, "utf-8");
				} catch (err) {
					const code = (err as NodeJS.ErrnoException).code;
					if (code !== "ENOENT")
						discoveryWarnings.push(
							`[scramjet/discovery] could not read ${recPath} (${code ?? "unknown"}: ${(err as Error).message})`,
						);
					continue;
				}
				try {
					const recs = parseAutonomyRecommendations(content, discoveryWarnings);
					recs.publications = filterPublicationDefaults(recs, registry, set.dir, discoveryWarnings);
					const prior = recommendations.get(set.name);
					if (prior?.publications) recs.publications = { ...prior.publications, ...recs.publications };
					if (Object.keys(recs.edges).length > 0 || Object.keys(recs.publications ?? {}).length > 0) {
						for (const warning of validateRecommendations(recs, registry)) discoveryWarnings.push(warning);
						recommendations.set(set.name, recs);
					}
				} catch (err) {
					discoveryWarnings.push(`[scramjet/discovery] could not parse ${recPath}: ${(err as Error).message}`);
				}
			}
			state.autonomyRecommendations = recommendations;

			for (const [scope, root] of [
				["global", globalDir],
				["project", projectDir],
			] as const) {
				for (const name of BUNDLED_SETS) {
					const legacyPath = join(root, name);
					const packageAvailable = packagedSets.get(name) !== undefined;
					const authority = packageAvailable
						? "package resources remain active and the legacy path was left untouched"
						: "package resources for this set are unavailable; reinstall Scramjet. The legacy path was left untouched and remains non-authoritative; no legacy fallback was used";
					let signature: string;
					let warning: string;
					try {
						const finding = legacyInspector({ legacyPath, packagePath: join(bundledRoot, name), scope });
						if (!finding?.actionable) continue;
						signature = `${packageAvailable}:${finding.signature}`;
						warning = formatLegacyBundleWarning(finding, packageAvailable);
					} catch {
						signature = `${packageAvailable}:inspection-failed:${resolve(legacyPath)}`;
						warning = `Could not inspect ignored legacy bundled command set at ${legacyPath}; ${authority}. Compare it manually before migration.`;
					}
					if (!journaledLegacySignatures.has(signature)) {
						state.logger.warn("discovery", warning);
						journaledLegacySignatures.add(signature);
					}
					if (ctx?.hasUI && interactiveOutput && !displayedLegacySignatures.has(signature)) {
						try {
							ctx.ui.notify(warning, "warning");
							displayedLegacySignatures.add(signature);
						} catch (err) {
							state.logger.warn("discovery", `could not display legacy migration warning for ${legacyPath}`, {
								legacyPath,
								cause: err instanceof Error ? err.message : String(err),
							});
						}
					}
				}
			}

			for (const warning of discoveryWarnings) state.logger.warn("discovery", warning);
			for (const warning of [...warnings, ...agentWarnings]) state.logger.warn("discovery", warning);

			const publicationWarnings = discoveryWarnings.filter((warning) => /publication/i.test(warning));
			const publicationWarningSignature = publicationWarnings.join("\0");
			if (
				publicationWarnings.length > 0 &&
				ctx?.hasUI &&
				interactiveOutput &&
				publicationWarningSignature !== lastPublicationWarningSignature
			)
				ctx.ui.notify(
					`Ignored ${publicationWarnings.length} publication default${publicationWarnings.length === 1 ? "" : "s"} from autonomy-defaults.yaml; affected publications will always ask. Review discovery warnings and reload Scramjet.`,
					"warning",
				);
			lastPublicationWarningSignature = publicationWarningSignature;

			return { skillPaths, promptPaths: [...registry.values()].map((def) => def.filePath), themePaths };
		} catch (err) {
			state.logger.warn(
				"discovery",
				`failed: ${(err as Error).message}; no scramjet commands will be available this session (bundled scramjet-dark theme unaffected)`,
			);
			return { skillPaths, promptPaths: [], themePaths };
		}
	});
}
