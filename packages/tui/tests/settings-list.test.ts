import { describe, expect, it, vi } from "vitest";
import { type SettingItem, SettingsList, type SettingsListTheme } from "../src/components/settings-list.js";
import { type Component, CURSOR_MARKER, type Focusable, TUI } from "../src/tui.js";
import { visibleWidth } from "../src/utils.js";
import { HeadlessTerminal } from "./helpers/headless-terminal.js";

const theme: SettingsListTheme = {
	label: (text) => text,
	value: (text) => text,
	description: (text) => text,
	cursor: "> ",
	hint: (text) => text,
};

function list(items: SettingItem[], enableSearch = true): SettingsList {
	return new SettingsList(
		items,
		10,
		theme,
		() => {},
		() => {},
		{ enableSearch },
	);
}

describe("SettingsList search", () => {
	it.each([false, true])(
		"activates literal and CSI-u Space with search=%s through normal dispatch",
		(enableSearch) => {
			const changed = vi.fn();
			const submenuInput = vi.fn();
			const settings = new SettingsList(
				[
					{ id: "alpha", label: "Alpha", currentValue: "off", values: ["off", "on"] },
					{ id: "gamma", label: "Gamma", currentValue: "off", values: ["off", "on"] },
					{
						id: "submenu",
						label: "Submenu",
						currentValue: "",
						submenu: () => ({ render: () => [], invalidate() {}, handleInput: submenuInput }),
					},
				],
				10,
				theme,
				changed,
				() => {},
				{ enableSearch },
			);
			const terminal = new HeadlessTerminal();
			const tui = new TUI(terminal);
			tui.addChild(settings);
			tui.setFocus(settings);
			tui.start();
			try {
				if (enableSearch) terminal.sendInput("g");
				for (const data of ["\x1b[32;2u", "\x1b[32;5u", "\x1b[32;1:3u"]) terminal.sendInput(data);
				expect(changed).not.toHaveBeenCalled();
				terminal.sendInput(" ");
				terminal.sendInput("\x1b[32u");
				expect(changed.mock.calls).toEqual([
					[enableSearch ? "gamma" : "alpha", "on"],
					[enableSearch ? "gamma" : "alpha", "off"],
				]);
				tui.setFocus(
					list(
						[
							{
								id: "submenu",
								label: "Submenu",
								currentValue: "",
								submenu: () => ({ render: () => [], invalidate() {}, handleInput: submenuInput }),
							},
						],
						enableSearch,
					),
				);
				terminal.sendInput("\r");
				terminal.sendInput("\x1b[32u");
				expect(submenuInput).toHaveBeenCalledExactlyOnceWith("\x1b[32u");
			} finally {
				tui.stop();
			}
		},
	);
	it("renders a discoverable search input and filters labels", () => {
		const settings = list([
			{ id: "alpha", label: "Alpha", currentValue: "on" },
			{ id: "gamma", label: "Gamma", currentValue: "off" },
		]);
		expect(settings.render(40).join("\n")).toContain("Type to search");
		settings.handleInput("g");
		const rendered = settings.render(40).join("\n");
		expect(rendered).toContain("Gamma");
		expect(rendered).not.toContain("Alpha");
		settings.handleInput("\x7f");
		expect(settings.render(40).join("\n")).toContain("Alpha");
	});

	it("activates the filtered item rather than the same unfiltered index", () => {
		const changes: string[] = [];
		const settings = new SettingsList(
			[
				{ id: "alpha", label: "Alpha", currentValue: "off", values: ["off", "on"] },
				{ id: "gamma", label: "Gamma", currentValue: "off", values: ["off", "on"] },
			],
			10,
			theme,
			(id) => changes.push(id),
			() => {},
			{ enableSearch: true },
		);
		settings.handleInput("g");
		settings.handleInput("\r");
		expect(changes).toEqual(["gamma"]);
	});

	it("propagates focus to the search input for the hardware cursor marker", () => {
		const settings = list([{ id: "alpha", label: "Alpha", currentValue: "on" }]);
		(settings as SettingsList & Focusable).focused = true;
		expect(settings.render(40).join("\n")).toContain(CURSOR_MARKER);
	});

	it("transfers focus into and back from a focusable submenu", () => {
		let submenu: (Component & Focusable) | undefined;
		const settings = list([
			{
				id: "alpha",
				label: "Alpha",
				currentValue: "on",
				submenu: (_value, done) => {
					submenu = {
						focused: false,
						render: () => ["submenu"],
						handleInput: (data) => {
							if (data === "\x1b") done();
						},
						invalidate() {},
					};
					return submenu;
				},
			},
		]);
		(settings as SettingsList & Focusable).focused = true;
		settings.handleInput("\r");
		expect(submenu?.focused).toBe(true);
		settings.handleInput("\x1b");
		expect(submenu?.focused).toBe(false);
		expect(settings.render(40).join("\n")).toContain(CURSOR_MARKER);
	});

	it("keeps every search and settings line within narrow widths", () => {
		const settings = list([
			{
				id: "long",
				label: "A very long publication command name",
				currentValue: "Follow command (Auto-approve)",
				description: "Effective publication policy remains visible in this wrapped description.",
			},
		]);
		const lines = settings.render(24);
		for (const line of lines) expect(visibleWidth(line)).toBeLessThanOrEqual(24);
		expect(lines.join(" ").replace(/\s+/g, " ")).toContain("Follow command (Auto-approve)");
	});
});
