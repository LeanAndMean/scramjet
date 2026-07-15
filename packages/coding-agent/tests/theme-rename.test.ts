import { describe, expect, it } from "vitest";
import { SettingsManager } from "../src/core/settings-manager.js";
import { getThemeByName, resolveThemeName } from "../src/modes/interactive/theme/theme.js";

describe("resolveThemeName", () => {
	it("returns the explicit setting verbatim", () => {
		expect(
			resolveThemeName({ explicitSetting: "my-theme", detectedClassification: "dark", hasScramjetDark: true }),
		).toBe("my-theme");
	});

	it("resolves a light classification to pi-light (never scramjet-dark)", () => {
		expect(
			resolveThemeName({ explicitSetting: undefined, detectedClassification: "light", hasScramjetDark: true }),
		).toBe("pi-light");
	});

	it("resolves a dark classification to scramjet-dark when available", () => {
		expect(
			resolveThemeName({ explicitSetting: undefined, detectedClassification: "dark", hasScramjetDark: true }),
		).toBe("scramjet-dark");
	});

	it("resolves a dark classification to pi-dark when scramjet-dark is unavailable", () => {
		expect(
			resolveThemeName({ explicitSetting: undefined, detectedClassification: "dark", hasScramjetDark: false }),
		).toBe("pi-dark");
	});

	it("treats an undefined classification as the dark branch", () => {
		expect(
			resolveThemeName({ explicitSetting: undefined, detectedClassification: undefined, hasScramjetDark: false }),
		).toBe("pi-dark");
		expect(
			resolveThemeName({ explicitSetting: undefined, detectedClassification: undefined, hasScramjetDark: true }),
		).toBe("scramjet-dark");
	});
});

describe("settings theme migration", () => {
	it('migrates "dark" to "pi-dark"', () => {
		expect(SettingsManager.inMemory({ theme: "dark" }).getTheme()).toBe("pi-dark");
	});

	it('migrates "light" to "pi-light"', () => {
		expect(SettingsManager.inMemory({ theme: "light" }).getTheme()).toBe("pi-light");
	});

	it("leaves custom theme names unchanged", () => {
		expect(SettingsManager.inMemory({ theme: "my-custom" }).getTheme()).toBe("my-custom");
	});

	it("leaves a name merely containing a legacy substring unchanged (exact-match guard)", () => {
		expect(SettingsManager.inMemory({ theme: "my-dark" }).getTheme()).toBe("my-dark");
		expect(SettingsManager.inMemory({ theme: "darkane" }).getTheme()).toBe("darkane");
		expect(SettingsManager.inMemory({ theme: "light-mode" }).getTheme()).toBe("light-mode");
	});

	it("is idempotent on an already-migrated value", () => {
		expect(SettingsManager.inMemory({ theme: "pi-dark" }).getTheme()).toBe("pi-dark");
		expect(SettingsManager.inMemory({ theme: "pi-light" }).getTheme()).toBe("pi-light");
	});

	it("leaves an absent theme undefined", () => {
		expect(SettingsManager.inMemory({}).getTheme()).toBeUndefined();
	});
});

describe("builtin theme rename", () => {
	it("loads pi-dark by its renamed name", () => {
		expect(getThemeByName("pi-dark")?.name).toBe("pi-dark");
	});

	it("loads pi-light by its renamed name", () => {
		expect(getThemeByName("pi-light")?.name).toBe("pi-light");
	});
});
