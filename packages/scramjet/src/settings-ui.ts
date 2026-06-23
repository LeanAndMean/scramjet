import type { ExtensionAPI, ExtensionContext } from "@scramjet/coding-agent";
import type { Component } from "@scramjet/tui";
import { type SettingItem, SettingsList, type SettingsListTheme } from "@scramjet/tui";
import { defaultConfigPath, loadAutonomyConfig, lookupEdge, saveAutonomyConfig } from "./autonomy-settings.js";
import { ENABLED_TOGGLE_TYPE, type EnabledToggleData } from "./history.js";
import type { AutonomyConfig, NextStepPolicy, ScramjetState } from "./types.js";

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
	onChange: (commandName: string, target: string, value: string) => void,
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
	onChange: (commandName: string, target: string, value: string) => void,
	onCancel: () => void,
): Component {
	const edgeItems = buildEdgeItems(commandName, policy, configGetter());
	const list = new SettingsList(
		edgeItems,
		Math.min(edgeItems.length, 10),
		theme,
		(id, newValue) => {
			const target = id.split("::")[1];
			onChange(commandName, target, newValue);
		},
		onCancel,
	);
	return list;
}

export function buildTopLevelItems(
	state: ScramjetState,
	configGetter: () => AutonomyConfig | null,
	theme: SettingsListTheme,
	commandOnChange: (commandName: string, target: string, value: string) => void,
): SettingItem[] {
	const items: SettingItem[] = [];

	items.push({
		id: "auto-continuation",
		label: "Scramjet auto-continuation",
		description: "When on, Scramjet automatically chains commands based on next-step policies",
		currentValue: state.enabled ? "on" : "off",
		values: ["on", "off"],
	});

	const commandsWithEdges = [...state.registry.values()].filter((def) => def.next != null);
	if (commandsWithEdges.length > 0) {
		const edgeSummary = buildRegistrySummary(configGetter());
		items.push({
			id: "command-autonomy",
			label: "Command autonomy",
			description: "Per-edge overrides for command chaining behavior",
			currentValue: edgeSummary,
			submenu: (_currentValue, done) => {
				const commandItems = buildCommandItems(state, configGetter, theme, commandOnChange);
				return new SettingsList(
					commandItems,
					Math.min(commandItems.length, 10),
					theme,
					(_id, _newValue) => {},
					() => done(buildRegistrySummary(configGetter())),
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
	const configGetter = () => safeLoadConfig(configPath, ctx);

	const handleAutonomyChange = (commandName: string, target: string, value: string) => {
		const current = configGetter() ?? { edges: {} };
		if (value === "default") {
			if (current.edges[commandName]) {
				delete current.edges[commandName][target];
				if (Object.keys(current.edges[commandName]).length === 0) {
					delete current.edges[commandName];
				}
			}
		} else {
			if (!current.edges[commandName]) current.edges[commandName] = {};
			current.edges[commandName][target] = value as "chain" | "pause";
		}
		try {
			saveAutonomyConfig(configPath, current);
		} catch (err: unknown) {
			const msg = err instanceof Error ? err.message : String(err);
			ctx.ui.notify(`Failed to save autonomy config: ${msg}`, "error");
		}
	};

	await ctx.ui.custom<void>((tui, theme, _keybindings, done) => {
		const settingsTheme = buildSettingsTheme(theme as Parameters<SettingsThemeFactory>[0]);

		const topItems = buildTopLevelItems(state, configGetter, settingsTheme, handleAutonomyChange);
		const list = new SettingsList(
			topItems,
			Math.min(topItems.length + 2, 10),
			settingsTheme,
			(id, newValue) => {
				if (id === "auto-continuation") {
					state.enabled = newValue === "on";
					const payload: EnabledToggleData = { enabled: state.enabled };
					pi.appendEntry(ENABLED_TOGGLE_TYPE, payload);
				}
			},
			() => done(),
		);

		return {
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

function safeLoadConfig(configPath: string, ctx: ExtensionContext): AutonomyConfig | null {
	try {
		return loadAutonomyConfig(configPath);
	} catch (err: unknown) {
		const msg = err instanceof Error ? err.message : String(err);
		ctx.ui.notify(`autonomy.yaml is corrupt or unreadable — starting with defaults: ${msg}`, "warning");
		return null;
	}
}
