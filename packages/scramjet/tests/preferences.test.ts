import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	DEFAULT_PREFERENCES,
	defaultPreferencesPath,
	loadPreferences,
	resetCache,
	savePreferences,
} from "../src/preferences.js";

describe("preferences", () => {
	let tmpDir: string;
	let configPath: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-prefs-test-"));
		configPath = path.join(tmpDir, "preferences.yaml");
		resetCache();
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	describe("defaultPreferencesPath", () => {
		it("uses XDG_CONFIG_HOME when set", () => {
			const prev = process.env.XDG_CONFIG_HOME;
			try {
				process.env.XDG_CONFIG_HOME = "/custom/config";
				expect(defaultPreferencesPath()).toBe("/custom/config/scramjet/preferences.yaml");
			} finally {
				if (prev === undefined) delete process.env.XDG_CONFIG_HOME;
				else process.env.XDG_CONFIG_HOME = prev;
			}
		});

		it("falls back to ~/.config", () => {
			const prev = process.env.XDG_CONFIG_HOME;
			try {
				delete process.env.XDG_CONFIG_HOME;
				expect(defaultPreferencesPath()).toBe(path.join(os.homedir(), ".config", "scramjet", "preferences.yaml"));
			} finally {
				if (prev === undefined) delete process.env.XDG_CONFIG_HOME;
				else process.env.XDG_CONFIG_HOME = prev;
			}
		});
	});

	describe("loadPreferences", () => {
		it("returns defaults on ENOENT", () => {
			expect(loadPreferences(configPath)).toEqual(DEFAULT_PREFERENCES);
		});

		it("parses valid YAML with correct booleans", () => {
			fs.writeFileSync(configPath, "title_indicator: false\nbell: true\n");
			expect(loadPreferences(configPath)).toEqual({ title_indicator: false, bell: true });
		});

		it("fills in missing keys with defaults", () => {
			fs.writeFileSync(configPath, "bell: true\n");
			expect(loadPreferences(configPath)).toEqual({ title_indicator: true, bell: true });
		});

		it("throws for invalid YAML", () => {
			fs.writeFileSync(configPath, "{{{{invalid");
			expect(() => loadPreferences(configPath)).toThrow("preferences.yaml: failed to load config:");
		});

		it("returns defaults for non-object YAML", () => {
			fs.writeFileSync(configPath, "just a string\n");
			expect(loadPreferences(configPath)).toEqual(DEFAULT_PREFERENCES);
		});

		it("returns defaults for array YAML", () => {
			fs.writeFileSync(configPath, "- one\n- two\n");
			expect(loadPreferences(configPath)).toEqual(DEFAULT_PREFERENCES);
		});

		it("ignores non-boolean values for known keys", () => {
			fs.writeFileSync(configPath, "title_indicator: 42\nbell: maybe\n");
			expect(loadPreferences(configPath)).toEqual(DEFAULT_PREFERENCES);
		});
	});

	describe("mtime cache", () => {
		it("returns cached result on second read without file change", () => {
			fs.writeFileSync(configPath, "bell: true\n");
			const first = loadPreferences(configPath);
			const second = loadPreferences(configPath);
			expect(first).toEqual(second);
			expect(second.bell).toBe(true);
		});

		it("re-reads when mtime changes", () => {
			fs.writeFileSync(configPath, "bell: true\n");
			loadPreferences(configPath);
			// Write with a new mtime (default)
			const future = new Date(Date.now() + 2000);
			fs.writeFileSync(configPath, "bell: false\n");
			fs.utimesSync(configPath, future, future);
			const result = loadPreferences(configPath);
			expect(result.bell).toBe(false);
		});

		it("resetCache forces re-read", () => {
			fs.writeFileSync(configPath, "bell: true\n");
			loadPreferences(configPath);
			// Overwrite same mtime
			const stat = fs.statSync(configPath);
			fs.writeFileSync(configPath, "bell: false\n");
			fs.utimesSync(configPath, stat.atime, stat.mtime);
			resetCache();
			const result = loadPreferences(configPath);
			expect(result.bell).toBe(false);
		});

		it("invalidates cache when file is deleted", () => {
			fs.writeFileSync(configPath, "bell: true\n");
			expect(loadPreferences(configPath).bell).toBe(true);
			fs.unlinkSync(configPath);
			expect(loadPreferences(configPath)).toEqual(DEFAULT_PREFERENCES);
		});
	});

	describe("savePreferences + round-trip", () => {
		it("creates parent directories", () => {
			const deepPath = path.join(tmpDir, "a", "b", "preferences.yaml");
			savePreferences(deepPath, { title_indicator: false, bell: true });
			resetCache();
			expect(loadPreferences(deepPath)).toEqual({ title_indicator: false, bell: true });
		});

		it("round-trips correctly", () => {
			const prefs = { title_indicator: false, bell: true };
			savePreferences(configPath, prefs);
			resetCache();
			expect(loadPreferences(configPath)).toEqual(prefs);
		});

		it("overwrites existing file", () => {
			savePreferences(configPath, { title_indicator: true, bell: false });
			savePreferences(configPath, { title_indicator: false, bell: true });
			resetCache();
			expect(loadPreferences(configPath)).toEqual({ title_indicator: false, bell: true });
		});
	});
});
