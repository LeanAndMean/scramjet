import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import {
	formatLegacyBundleWarning,
	inspectLegacyBundle,
	type LegacyBundleInspection,
} from "../src/commands/legacy-bundle-inspection.js";

const sandboxes: string[] = [];

function sandbox(): string {
	const path = mkdtempSync(join(tmpdir(), "scramjet-legacy-inspection-"));
	sandboxes.push(path);
	return path;
}

function write(path: string, content: string): string {
	mkdirSync(path.slice(0, path.lastIndexOf("/")), { recursive: true });
	writeFileSync(path, content);
	return path;
}

function hash(content: string): string {
	return createHash("sha256").update(content).digest("hex");
}

function manifest(path: string, files: Record<string, string>, version = "0.43.5"): void {
	write(join(path, ".seed-manifest.json"), `${JSON.stringify({ version, files })}\n`);
}

afterEach(() => {
	for (const path of sandboxes.splice(0)) rmSync(path, { recursive: true, force: true });
});

describe("inspectLegacyBundle", () => {
	it("classifies valid-manifest changes even when its version matches the current package", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		const packagePath = join(root, "package", "mach12");
		write(join(packagePath, "commands", "mach12:clean.md"), "clean");
		write(join(packagePath, "commands", "mach12:new.md"), "package new");
		write(join(legacyPath, "commands", "mach12:clean.md"), "clean");
		write(join(legacyPath, "commands", "mach12:modified.md"), "locally changed");
		write(join(legacyPath, "commands", "mach12:new.md"), "pre-existing collision");
		write(join(legacyPath, "agents", "wrong-filename.md"), "local agent");
		write(join(legacyPath, "agents", "mach12:wrong-name.md"), "local agent two");
		manifest(
			legacyPath,
			{
				"commands/mach12:clean.md": hash("clean"),
				"commands/mach12:modified.md": hash("original"),
				"commands/mach12:removed.md": hash("removed"),
			},
			"current",
		);

		const result = inspectLegacyBundle({ legacyPath, packagePath, scope: "global" });

		expect(result).toMatchObject({
			legacyPath,
			scope: "global",
			manifestVersion: "current",
			modified: ["commands/mach12:modified.md"],
			removed: ["commands/mach12:removed.md"],
			localOnly: ["agents/mach12:wrong-name.md", "agents/wrong-filename.md"],
			ambiguous: ["commands/mach12:new.md"],
			errors: [],
			actionable: true,
		});
		expect(result?.signature).toMatch(/^[a-f0-9]{64}$/);
	});

	it("returns a quiet result for a clean valid seed", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "scramjet");
		const packagePath = join(root, "package", "scramjet");
		write(join(legacyPath, "commands", "scramjet:test.md"), "same");
		write(join(packagePath, "commands", "scramjet:test.md"), "same");
		manifest(legacyPath, { "commands/scramjet:test.md": hash("same") });

		expect(inspectLegacyBundle({ legacyPath, packagePath, scope: "global" })).toMatchObject({
			actionable: false,
			modified: [],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [],
		});
	});

	it.each([
		["traversal", { version: "1", files: { "../outside": "a".repeat(64) } }],
		["absolute", { version: "1", files: { "/outside": "a".repeat(64) } }],
		["Windows absolute", { version: "1", files: { "C:/outside": "a".repeat(64) } }],
		["backslash", { version: "1", files: { "commands\\bad.md": "a".repeat(64) } }],
		["NUL", { version: "1", files: { "commands/bad\0.md": "a".repeat(64) } }],
		["non-normalized", { version: "1", files: { "commands//bad.md": "a".repeat(64) } }],
		["non-string path hash", { version: "1", files: { "commands/bad.md": 42 } }],
		["invalid hash", { version: "1", files: { "commands/bad.md": "nope" } }],
		["non-string version", { version: 1, files: {} }],
		["missing files", { version: "1" }],
	] as const)("treats a %s manifest as uncertain without making edit claims", (_case, value) => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		const packagePath = join(root, "package", "mach12");
		write(join(legacyPath, "commands", "mach12:test.md"), "legacy");
		write(join(packagePath, "commands", "mach12:test.md"), "package");
		write(join(legacyPath, ".seed-manifest.json"), JSON.stringify(value));

		const result = inspectLegacyBundle({ legacyPath, packagePath, scope: "global" });

		expect(result).toMatchObject({
			modified: [],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [{ code: "manifest-invalid" }],
			actionable: true,
		});
	});

	it("reports missing provenance without claiming the files are edited", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		write(join(legacyPath, "commands", "mach12:test.md"), "legacy");

		expect(
			inspectLegacyBundle({ legacyPath, packagePath: join(root, "package", "mach12"), scope: "global" }),
		).toMatchObject({
			modified: [],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [{ code: "manifest-missing" }],
			actionable: true,
		});
	});

	it("makes no edit claims when the package inventory cannot be read", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		write(join(legacyPath, "commands", "mach12:custom.md"), "custom");
		manifest(legacyPath, {});

		expect(
			inspectLegacyBundle({ legacyPath, packagePath: join(root, "missing-package"), scope: "global" }),
		).toMatchObject({
			modified: [],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [{ code: "package-read-failed" }],
		});
	});

	it("does not follow top-level or nested legacy symlinks", () => {
		const root = sandbox();
		const target = join(root, "target");
		write(join(target, "secret.md"), "secret");
		const topLevel = join(root, "top-level");
		symlinkSync(target, topLevel);
		const topResult = inspectLegacyBundle({
			legacyPath: topLevel,
			packagePath: join(root, "package", "mach12"),
			scope: "global",
		});
		expect(topResult).toMatchObject({ errors: [{ code: "top-level-symlink" }], modified: [], localOnly: [] });

		const nested = join(root, "nested");
		const packagePath = join(root, "package", "mach12");
		mkdirSync(nested);
		mkdirSync(packagePath, { recursive: true });
		symlinkSync(target, join(nested, "linked"));
		manifest(nested, { "linked/managed.md": hash("managed") });
		const nestedResult = inspectLegacyBundle({
			legacyPath: nested,
			packagePath,
			scope: "global",
		});
		expect(nestedResult).toMatchObject({
			errors: [{ code: "legacy-symlink", path: "linked" }],
			modified: [],
			localOnly: [],
		});
		expect(readFileSync(join(target, "secret.md"), "utf8")).toBe("secret");
	});

	it("treats a reserved project tree as user-created without requiring seed provenance", () => {
		const root = sandbox();
		const legacyPath = join(root, "project", ".scramjet", "mach12");
		write(join(legacyPath, "commands", "mach12:custom.md"), "custom");

		expect(
			inspectLegacyBundle({ legacyPath, packagePath: join(root, "package", "mach12"), scope: "project" }),
		).toMatchObject({
			errors: [{ code: "reserved-project-tree" }],
			actionable: true,
			modified: [],
			localOnly: [],
		});
	});

	it("returns undefined when the legacy path does not exist", () => {
		const root = sandbox();
		expect(
			inspectLegacyBundle({
				legacyPath: join(root, "missing"),
				packagePath: join(root, "package", "mach12"),
				scope: "global",
			}),
		).toBeUndefined();
	});

	it("classifies object-prototype filenames as local-only", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		const packagePath = join(root, "package", "mach12");
		write(join(legacyPath, "constructor"), "custom");
		mkdirSync(packagePath, { recursive: true });
		manifest(legacyPath, {});

		expect(inspectLegacyBundle({ legacyPath, packagePath, scope: "global" })?.localOnly).toEqual(["constructor"]);
	});

	it("builds the same signature regardless of directory iteration order", () => {
		const root = sandbox();
		const legacyPath = join(root, "legacy", "mach12");
		const packagePath = join(root, "package", "mach12");
		write(join(legacyPath, "z.md"), "z");
		write(join(legacyPath, "a.md"), "a");
		mkdirSync(packagePath, { recursive: true });
		manifest(legacyPath, {});
		const ascending = inspectLegacyBundle(
			{ legacyPath, packagePath, scope: "global" },
			{ orderEntries: (entries) => [...entries].sort((a, b) => a.name.localeCompare(b.name)) },
		);
		const descending = inspectLegacyBundle(
			{ legacyPath, packagePath, scope: "global" },
			{ orderEntries: (entries) => [...entries].sort((a, b) => b.name.localeCompare(a.name)) },
		);

		expect(ascending?.localOnly).toEqual(["a.md", "z.md"]);
		expect(descending?.localOnly).toEqual(ascending?.localOnly);
		expect(descending?.signature).toBe(ascending?.signature);
	});
});

describe("formatLegacyBundleWarning", () => {
	it("distinguishes proven findings from uncertainty and gives complete fork guidance", () => {
		const inspection: LegacyBundleInspection = {
			legacyPath: "/legacy/mach12",
			scope: "global",
			manifestVersion: "0.43.5",
			modified: ["commands/mach12:edited.md"],
			removed: [],
			localOnly: ["agents/bad.md"],
			ambiguous: ["commands/mach12:new.md"],
			errors: [{ code: "legacy-symlink", path: "agents/link.md" }],
			actionable: true,
			signature: "signature",
		};

		const warning = formatLegacyBundleWarning(inspection, true);

		expect(warning).toContain("/legacy/mach12");
		expect(warning).toContain("package resources are active");
		expect(warning).toContain("left untouched");
		expect(warning).toContain("Proven local changes");
		expect(warning).toContain("Uncertain");
		expect(warning).toContain("different set name");
		expect(warning).toContain("rename command filenames");
		expect(warning).toContain("agent filenames");
		expect(warning).toContain("frontmatter names");
		expect(warning).toContain("delegation and next-step references");
		expect(warning).toContain("references that invoke copied agents");
		expect(warning).toContain("references to uncopied package agents may remain");
		expect(warning).toContain("default keys");
		expect(warning).toContain("command lint checks commands only");
		expect(warning).toContain("reload Scramjet to validate agents");
		expect(warning).not.toContain("local agent");
	});

	it("omits uncertainty guidance for proven-only findings", () => {
		const inspection: LegacyBundleInspection = {
			legacyPath: "/legacy/mach12",
			scope: "global",
			manifestVersion: "0.43.5",
			modified: ["commands/mach12:edited.md"],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [],
			actionable: true,
			signature: "signature",
		};

		const warning = formatLegacyBundleWarning(inspection, true);

		expect(warning).toContain("Proven local changes");
		expect(warning).not.toContain("Uncertain");
		expect(warning).not.toContain("Compare manually");
	});

	it("does not claim rejected package resources are active", () => {
		const inspection: LegacyBundleInspection = {
			legacyPath: "/legacy/mach12",
			scope: "global",
			modified: [],
			removed: [],
			localOnly: [],
			ambiguous: [],
			errors: [{ code: "manifest-missing" }],
			actionable: true,
			signature: "signature",
		};

		const warning = formatLegacyBundleWarning(inspection, false);

		expect(warning).toContain("package resources for this set are unavailable");
		expect(warning).toContain("reinstall Scramjet");
		expect(warning).toContain("remains non-authoritative");
		expect(warning).not.toContain("package resources are active");
	});
});
