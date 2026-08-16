import { describe, expect, it } from "vitest";
import { type SettingItem, SettingsList, type SettingsListTheme } from "../src/components/settings-list.js";
import { type Component, CURSOR_MARKER, type Focusable } from "../src/tui.js";
import { visibleWidth } from "../src/utils.js";

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
