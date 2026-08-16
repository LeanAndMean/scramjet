import type { ExtensionAPI, ExtensionContext } from "@leanandmean/coding-agent";
import type { Component } from "@leanandmean/tui";
import { Container, type SettingItem, SettingsList, type SettingsListTheme, Text } from "@leanandmean/tui";
import {
	applyRecommendations,
	defaultConfigPath,
	loadAutonomyConfigResult,
	lookupEdge,
	mergeAllRecommendations,
	resolvePublicationPolicy,
	updateAutonomyConfig,
} from "./autonomy-settings.js";
import { ENABLED_TOGGLE_TYPE, type EnabledToggleData } from "./history.js";
import { DEFAULT_PREFERENCES, loadPreferences, type Preferences, savePreferences } from "./preferences.js";
import type {
	AutonomyConfig,
	AutonomyRecommendations,
	NextStepPolicy,
	PublicationTool,
	RecommendationSetting,
	ScramjetState,
} from "./types.js";
import { PUBLICATION_TOOLS } from "./types.js";

export interface EdgeClassification {
	source: string;
	target: string;
	setting: RecommendationSetting;
	status: "pending" | "configured";
}

export function classifyRecommendationEdges(
	merged: AutonomyRecommendations,
	config: AutonomyConfig | null,
): EdgeClassification[] {
	const result: EdgeClassification[] = [];
	for (const [source, targets] of Object.entries(merged.edges)) {
		for (const [target, setting] of Object.entries(targets)) {
			if (setting === "default") continue;
			const status = config?.edges[source]?.[target] != null ? "configured" : "pending";
			result.push({ source, target, setting, status });
		}
	}
	return result;
}

const EDGE_VALUES = ["default", "chain", "pause"] as const;

type SettingsThemeFactory = (theme: {
	fg: (color: string, text: string) => string;
	bold: (text: string) => string;
}) => SettingsListTheme;

const buildSettingsTheme: SettingsThemeFactory = (theme) => ({
	label: (text, selected) => (selected ? theme.fg("accent", text) : text),
	value: (text, selected) => (selected ? theme.fg("accent", theme.bold(text)) : theme.fg("muted", text)),
	description: (text) => theme.fg("muted", text),
	cursor: theme.fg("accent", "› "),
	hint: (text) => theme.fg("dim", text),
});

export function buildEdgeItems(
	commandName: string,
	policy: NextStepPolicy,
	config: AutonomyConfig | null,
): SettingItem[] {
	const targets = resolveTargets(policy);
	return targets.map((target) => {
		const exactSetting = config?.edges[commandName]?.[target.name] ?? null;
		const effectiveSetting = lookupEdge(config, commandName, target.name);
		const wildcardInherited = !target.forced && exactSetting == null && effectiveSetting != null;
		const description = target.forced
			? "Forced transition — autonomy settings are ignored"
			: wildcardInherited
				? `Inherited wildcard override: ${effectiveSetting}; choose chain or pause to create an exact override`
				: `Override: chain (always run), pause (always ask), default (use policy)`;
		return {
			id: `${commandName}::${target.name}`,
			label: target.name,
			description,
			currentValue: effectiveSetting ?? "default",
			values: target.forced ? undefined : wildcardInherited ? ["chain", "pause"] : [...EDGE_VALUES],
		};
	});
}

interface ResolvedTarget {
	name: string;
	forced: boolean;
}

function resolveTargets(policy: NextStepPolicy): ResolvedTarget[] {
	switch (policy.mode) {
		case "forced":
			return [{ name: policy.target, forced: true }];
		case "closed":
		case "open":
			return policy.candidates.map((c) => ({ name: c.name, forced: false }));
		case "ask":
			return [];
	}
}

export function buildCommandItems(
	state: ScramjetState,
	configGetter: () => AutonomyConfig | null,
	theme: SettingsListTheme,
	onChange: (commandName: string, target: string, value: string) => boolean | undefined,
): SettingItem[] {
	const items: SettingItem[] = [];
	const sorted = [...state.registry.entries()]
		.filter(([, def]) => def.next != null)
		.sort(([a], [b]) => a.localeCompare(b));

	for (const [name, def] of sorted) {
		const edgeSummary = summarizeEdges(name, def.next!, configGetter());
		items.push({
			id: name,
			label: name,
			description: def.description,
			currentValue: edgeSummary,
			submenu: (_currentValue, done) =>
				buildEdgeSubmenu(name, def.next!, configGetter, theme, onChange, () =>
					done(summarizeEdges(name, def.next!, configGetter())),
				),
		});
	}
	return items;
}

function summarizeEdges(commandName: string, policy: NextStepPolicy, config: AutonomyConfig | null): string {
	const targets = resolveTargets(policy);
	if (targets.length === 0) return `${policy.mode} (no targets)`;
	const configurableTargets = targets.filter((t) => !t.forced);
	const exactOverrideCount = configurableTargets.filter((t) => config?.edges[commandName]?.[t.name] != null).length;
	const wildcardCount = configurableTargets.filter(
		(t) => config?.edges[commandName]?.[t.name] == null && lookupEdge(config, commandName, t.name) != null,
	).length;
	if (exactOverrideCount === 0 && wildcardCount === 0) {
		return `${policy.mode} · ${targets.length} edge${targets.length > 1 ? "s" : ""}`;
	}
	const parts: string[] = [];
	if (exactOverrideCount > 0) parts.push(`${exactOverrideCount}/${targets.length} overridden`);
	if (wildcardCount > 0) parts.push(`${wildcardCount}/${targets.length} wildcard`);
	return `${policy.mode} · ${parts.join(", ")}`;
}

function buildEdgeSubmenu(
	commandName: string,
	policy: NextStepPolicy,
	configGetter: () => AutonomyConfig | null,
	theme: SettingsListTheme,
	onChange: (commandName: string, target: string, value: string) => boolean | undefined,
	onCancel: () => void,
): Component {
	const edgeItems = buildEdgeItems(commandName, policy, configGetter());
	const list = new SettingsList(
		edgeItems,
		Math.min(edgeItems.length, 10),
		theme,
		(id, newValue) => {
			const target = id.split("::")[1];
			if (onChange(commandName, target, newValue) === false)
				list.updateValue(id, lookupEdge(configGetter(), commandName, target) ?? "default");
		},
		onCancel,
	);
	return list;
}

export function buildApplyRecommendationsItem(
	recommendations: ReadonlyMap<string, AutonomyRecommendations>,
	configGetter: () => AutonomyConfig | null,
	configPath: string,
	exitSubmenu: () => void,
	notify: (message: string, type?: "info" | "warning" | "error") => void,
	theme: SettingsListTheme,
): SettingItem | null {
	if (recommendations.size === 0) return null;

	const merged = mergeAllRecommendations(recommendations);
	const initialEdges = classifyRecommendationEdges(merged, configGetter());
	if (initialEdges.length === 0) return null;

	const unapplied = initialEdges.filter((e) => e.status === "pending").length;
	const label =
		unapplied > 0 ? `Apply recommended settings (${unapplied} pending)` : "Recommended settings (all applied)";

	return {
		id: "apply-recommendations",
		label,
		currentValue: "",
		submenu: (_currentValue, done) => {
			const freshEdges = classifyRecommendationEdges(merged, configGetter());
			const pending = freshEdges.filter((e) => e.status === "pending").length;

			const edgeItems: SettingItem[] = freshEdges.map((edge) => ({
				id: `rec-${edge.source}::${edge.target}`,
				label: `${edge.source} → ${edge.target}`,
				currentValue: `${edge.setting} (${edge.status})`,
			}));

			if (pending > 0) {
				edgeItems.push({
					id: "confirm-apply",
					label: `Apply ${pending} pending`,
					currentValue: "",
					values: ["apply"],
				});
			}

			return new SettingsList(
				edgeItems,
				Math.min(edgeItems.length + 2, 10),
				theme,
				(id, _newValue) => {
					if (id === "confirm-apply") {
						try {
							const result = applyRecommendations(configPath, merged);
							notify(`Applied ${result.applied} recommended setting${result.applied !== 1 ? "s" : ""}`, "info");
						} catch (err: unknown) {
							const msg = err instanceof Error ? err.message : String(err);
							notify(`Failed to apply recommendations: ${msg}`, "error");
						}
						exitSubmenu();
					}
				},
				() => done(),
			);
		},
	};
}

export function buildPublicationCommandItems(
	state: ScramjetState,
	configGetter: () => AutonomyConfig | null,
	theme: SettingsListTheme,
	onChange: (commandName: string, tool: PublicationTool, value: string) => boolean | undefined,
	configInvalid: () => boolean = () => false,
): SettingItem[] {
	const defaults = mergeAllRecommendations(state.autonomyRecommendations);
	return [...state.registry.entries()]
		.filter(([, definition]) => !definition.delegateOnly)
		.sort(([left], [right]) => left.localeCompare(right))
		.flatMap(([commandName, definition]) => {
			const tools = PUBLICATION_TOOLS.filter((tool) => definition.allowedTools?.includes(tool) === true);
			if (tools.length === 0) return [];
			const summary = () => {
				const config = configGetter();
				return configInvalid()
					? "Always ask (config error)"
					: summarizePublicationPolicies(config, defaults, commandName, tools);
			};
			const commandItem: SettingItem = {
				id: `publication-command::${commandName}`,
				label: commandName,
				description: publicationCommandDescription(definition.description, summary()),
				currentValue: summary(),
				submenu: (_currentValue: string, done: (value?: string) => void) => {
					const config = configGetter();
					const invalid = configInvalid();
					const items = tools.map((tool) => {
						const override = config?.publications?.[commandName]?.[tool];
						const commandDefault = defaults.publications?.[commandName]?.[tool] ?? "require-approval";
						const defaultLabel = friendlyPublicationPolicy(commandDefault);
						const followCommand = `Follow command (${defaultLabel})`;
						return {
							id: `${commandName}::${tool}`,
							label: tool,
							description: invalid
								? "autonomy.yaml is invalid; publication safely requires approval until the file is fixed"
								: `${followCommand} uses the command-set author's setting; publication verification remains active`,
							currentValue: invalid
								? "Always ask (config error)"
								: override === "always-ask"
									? "Always ask"
									: override === "auto-approve"
										? "Auto-approve"
										: followCommand,
							values: invalid ? undefined : ["Always ask", followCommand, "Auto-approve"],
							confirmValue: invalid
								? undefined
								: (value: string, confirm: (confirmed: boolean) => void) =>
										value === "Auto-approve" || (value === followCommand && commandDefault === "auto-approve")
											? publicationAutoApproveConfirmation(commandName, tool, confirm, theme)
											: undefined,
						};
					});
					const list = new SettingsList(
						items,
						Math.min(items.length + 2, 10),
						theme,
						(id, value) => {
							const separator = id.lastIndexOf("::");
							const tool = id.slice(separator + 2) as PublicationTool;
							const commandDefault = defaults.publications?.[commandName]?.[tool] ?? "require-approval";
							const followCommand = `Follow command (${friendlyPublicationPolicy(commandDefault)})`;
							const stored =
								value === followCommand
									? onChange(commandName, tool, "follow-command")
									: value === "Always ask"
										? onChange(commandName, tool, "always-ask")
										: value === "Auto-approve"
											? onChange(commandName, tool, "auto-approve")
											: undefined;
							if (stored === false) {
								const freshOverride = configGetter()?.publications?.[commandName]?.[tool];
								list.updateValue(
									id,
									configInvalid()
										? "Always ask (config error)"
										: freshOverride === "always-ask"
											? "Always ask"
											: freshOverride === "auto-approve"
												? "Auto-approve"
												: followCommand,
								);
							}
						},
						() => {
							const refreshed = summary();
							commandItem.description = publicationCommandDescription(definition.description, refreshed);
							done(refreshed);
						},
					);
					return list;
				},
			};
			return [commandItem];
		});
}

function publicationAutoApproveConfirmation(
	commandName: string,
	tool: PublicationTool,
	done: (confirmed: boolean) => void,
	theme: SettingsListTheme,
): Component {
	const container = new Container();
	container.addChild(
		new Text(
			`Future ${tool} publications for ${commandName} will skip the approval card. Validation and exact verification remain active.`,
			0,
			1,
		),
	);
	const choices = new SettingsList(
		[
			{ id: "keep", label: "Keep current policy", currentValue: "", values: ["select"] },
			{ id: "enable", label: "Enable Auto-approve", currentValue: "", values: ["select"] },
		],
		2,
		theme,
		(id) => done(id === "enable"),
		() => done(false),
	);
	container.addChild(choices);
	return {
		render: (width: number) => container.render(width),
		invalidate: () => container.invalidate(),
		handleInput: (data: string) => choices.handleInput(data),
	};
}

function friendlyPublicationPolicy(policy: "require-approval" | "auto-approve"): string {
	return policy === "require-approval" ? "Always ask" : "Auto-approve";
}

function summarizePublicationPolicies(
	config: AutonomyConfig | null,
	defaults: AutonomyRecommendations,
	commandName: string,
	tools: PublicationTool[],
): string {
	const policies = tools.map((tool) => resolvePublicationPolicy(config, defaults, commandName, tool).policy);
	const alwaysAsk = policies.filter((policy) => policy === "require-approval").length;
	if (alwaysAsk === policies.length) return "Always ask";
	if (alwaysAsk === 0) return "Auto-approve";
	return `${alwaysAsk} Always ask · ${policies.length - alwaysAsk} Auto-approve`;
}

function publicationCommandDescription(description: string | undefined, summary: string): string {
	return `${description ? `${description} · ` : ""}Effective publication policy: ${summary}`;
}

export function buildTopLevelItems(
	state: ScramjetState,
	configGetter: () => AutonomyConfig | null,
	theme: SettingsListTheme,
	commandOnChange: (commandName: string, target: string, value: string) => boolean | undefined,
	configPath?: string,
	notify?: (message: string, type?: "info" | "warning" | "error") => void,
	publicationOnChange?: (commandName: string, tool: PublicationTool, value: string) => boolean | undefined,
	configInvalid: () => boolean = () => false,
): SettingItem[] {
	const items: SettingItem[] = [];

	items.push({
		id: "auto-continuation",
		label: "Autopilot",
		description: "When on, Scramjet automatically chains commands based on next-step policies (/autopilot on|off)",
		currentValue: state.enabled ? "on" : "off",
		values: ["on", "off"],
	});

	let prefs: Preferences;
	try {
		prefs = loadPreferences(state.preferencesPath);
	} catch {
		prefs = { ...DEFAULT_PREFERENCES };
	}
	items.push({
		id: "title-indicator",
		label: "Title indicator",
		description: "Show ● working / ○ waiting prefix in the terminal title",
		currentValue: prefs.title_indicator ? "on" : "off",
		values: ["on", "off"],
	});
	items.push({
		id: "terminal-bell",
		label: "Terminal bell",
		description: "Ring terminal bell when the agent is waiting for input",
		currentValue: prefs.bell ? "on" : "off",
		values: ["on", "off"],
	});

	const commandsWithEdges = [...state.registry.values()].filter((def) => def.next != null);
	if (commandsWithEdges.length > 0) {
		const edgeSummary = buildRegistrySummary(configGetter());
		const invalid = configInvalid();
		items.push({
			id: "command-autonomy",
			label: "Command autonomy",
			description: invalid
				? "autonomy.yaml is invalid; fix it before editing command autonomy"
				: "Per-edge overrides for command chaining behavior",
			currentValue: invalid ? "config error" : edgeSummary,
			submenu: invalid
				? undefined
				: (_currentValue, done) => {
						const closeSummary = () => done(buildRegistrySummary(configGetter()));
						const commandItems = buildCommandItems(state, configGetter, theme, commandOnChange);
						const applyItem =
							configPath && notify
								? buildApplyRecommendationsItem(
										state.autonomyRecommendations,
										configGetter,
										configPath,
										closeSummary,
										notify,
										theme,
									)
								: null;
						const allItems = applyItem ? [applyItem, ...commandItems] : commandItems;
						return new SettingsList(
							allItems,
							Math.min(allItems.length, 10),
							theme,
							(_id, _newValue) => {},
							closeSummary,
						);
					},
		});
	} else {
		items.push({
			id: "command-autonomy",
			label: "Command autonomy",
			description: "No commands with next-step policies registered — load a command set to configure edges",
			currentValue: "no edges",
		});
	}

	const publicationCommands = publicationOnChange
		? buildPublicationCommandItems(state, configGetter, theme, publicationOnChange, configInvalid)
		: [];
	if (publicationCommands.length > 0 && publicationOnChange) {
		items.push({
			id: "publication-approval",
			label: "Publication approval",
			description: "Per-command approval behavior for eligible forge publication tools",
			currentValue: `${publicationCommands.length} command${publicationCommands.length === 1 ? "" : "s"}`,
			submenu: (_currentValue, done) => {
				const commandItems = buildPublicationCommandItems(
					state,
					configGetter,
					theme,
					publicationOnChange,
					configInvalid,
				);
				return new SettingsList(
					commandItems,
					Math.min(commandItems.length, 10),
					theme,
					() => {},
					() => done(),
					{ enableSearch: commandItems.length > 10 },
				);
			},
		});
	}

	return items;
}

function buildRegistrySummary(config: AutonomyConfig | null): string {
	const totalOverrides = config
		? Object.values(config.edges).reduce((sum, targets) => sum + Object.keys(targets).length, 0)
		: 0;
	if (totalOverrides === 0) return "all defaults";
	return `${totalOverrides} override${totalOverrides > 1 ? "s" : ""}`;
}

export async function showSettingsPage(pi: ExtensionAPI, ctx: ExtensionContext, state: ScramjetState): Promise<void> {
	const configPath = state.autonomyConfigPath || defaultConfigPath();
	let configInvalid = false;
	let lastConfigError = "";
	const configGetter = () => {
		const result = loadAutonomyConfigResult(configPath);
		configInvalid = result.status === "invalid";
		if (result.status === "invalid") {
			if (result.error.message !== lastConfigError) {
				lastConfigError = result.error.message;
				ctx.ui.notify(
					`autonomy.yaml is corrupt or unreadable — approval is required: ${result.error.message}`,
					"warning",
				);
			}
			return null;
		}
		lastConfigError = "";
		return result.status === "valid" ? result.config : null;
	};

	const handleAutonomyChange = (commandName: string, target: string, value: string): boolean => {
		try {
			updateAutonomyConfig(configPath, (current) => {
				if (value === "default") {
					if (!current.edges[commandName]?.[target]) return false;
					delete current.edges[commandName][target];
					if (Object.keys(current.edges[commandName]).length === 0) delete current.edges[commandName];
				} else {
					current.edges[commandName] ??= {};
					if (current.edges[commandName][target] === value) return false;
					current.edges[commandName][target] = value as "chain" | "pause";
				}
				return true;
			});
			return true;
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			ctx.ui.notify(`Failed to save autonomy config: ${msg}`, "error");
			return false;
		}
	};

	const handlePublicationChange = (commandName: string, tool: PublicationTool, value: string): boolean => {
		try {
			updateAutonomyConfig(configPath, (current) => {
				if (value === "follow-command") {
					if (!current.publications?.[commandName]?.[tool]) return false;
					delete current.publications[commandName][tool];
					if (Object.keys(current.publications[commandName]).length === 0)
						delete current.publications[commandName];
				} else {
					current.publications ??= {};
					current.publications[commandName] ??= {};
					if (current.publications[commandName][tool] === value) return false;
					current.publications[commandName][tool] = value as "always-ask" | "auto-approve";
				}
				return true;
			});
			return true;
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			ctx.ui.notify(`Failed to save autonomy config: ${msg}`, "error");
			return false;
		}
	};

	await ctx.ui.custom<void>((tui, theme, _keybindings, done) => {
		const settingsTheme = buildSettingsTheme(theme as Parameters<SettingsThemeFactory>[0]);

		const topItems = buildTopLevelItems(
			state,
			configGetter,
			settingsTheme,
			handleAutonomyChange,
			configPath,
			(msg, type) => ctx.ui.notify(msg, type),
			handlePublicationChange,
			() => configInvalid,
		);
		const list = new SettingsList(
			topItems,
			Math.min(topItems.length + 2, 10),
			settingsTheme,
			(id, newValue) => {
				if (id === "auto-continuation") {
					state.enabled = newValue === "on";
					const payload: EnabledToggleData = { enabled: state.enabled };
					pi.appendEntry(ENABLED_TOGGLE_TYPE, payload);
				} else if (id === "title-indicator" || id === "terminal-bell") {
					try {
						const currentPrefs = loadPreferences(state.preferencesPath);
						const key = id === "title-indicator" ? "title_indicator" : "bell";
						currentPrefs[key] = newValue === "on";
						savePreferences(state.preferencesPath, currentPrefs);
					} catch (err: unknown) {
						const msg = err instanceof Error ? err.message : String(err);
						ctx.ui.notify(`Failed to save preferences: ${msg}`, "error");
					}
				}
			},
			() => done(),
		);

		return {
			get focused() {
				return list.focused;
			},
			set focused(value: boolean) {
				list.focused = value;
			},
			render(width: number) {
				return list.render(width);
			},
			invalidate() {
				list.invalidate();
			},
			handleInput(data: string) {
				list.handleInput(data);
				tui.requestRender();
			},
			dispose() {},
		};
	});
}
