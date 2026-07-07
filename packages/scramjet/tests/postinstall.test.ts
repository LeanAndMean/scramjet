/**
 * Postinstall integration tests for bundled-tree seeding, manifest-based
 * upgrades, legacy migration, and stale extension cleanup.
 *
 * scripts/postinstall.js resolves its source by computing pkgRoot from
 * import.meta.url (relative to the script file), so re-running the *real*
 * scripts/postinstall.js with a temp XDG_DATA_HOME is the closest thing to
 * a true integration test we can write without an actual `npm install`.
 */

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	cpSync,
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readdirSync,
	readFileSync,
	rmSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..");
const REAL_SCRIPT = join(REPO_ROOT, "scripts", "postinstall.js");
const OLD_SUBAGENT_EXAMPLE = resolve(REPO_ROOT, "..", "coding-agent", "examples", "extensions", "subagent");
const PKG_VERSION = JSON.parse(readFileSync(join(REPO_ROOT, "package.json"), "utf-8")).version;
const MANIFEST_NAME = ".seed-manifest.json";

interface RunResult {
	stdout: string;
	stderr: string;
	status: number;
}

function runScript(scriptPath: string, env: NodeJS.ProcessEnv): RunResult {
	const res = spawnSync(process.execPath, [scriptPath], {
		env: { ...process.env, ...env },
		encoding: "utf-8",
	});
	return {
		stdout: res.stdout ?? "",
		stderr: res.stderr ?? "",
		status: res.status ?? -1,
	};
}

function pathExists(p: string): boolean {
	try {
		lstatSync(p);
		return true;
	} catch (err) {
		if ((err as NodeJS.ErrnoException).code === "ENOENT") return false;
		throw err;
	}
}

function sha256(filePath: string): string {
	const content = readFileSync(filePath);
	return createHash("sha256").update(content).digest("hex");
}

function readManifest(destDir: string): { version: string; files: Record<string, string> } {
	return JSON.parse(readFileSync(join(destDir, MANIFEST_NAME), "utf-8"));
}

function writeManifest(destDir: string, manifest: { version: string; files: Record<string, string> }): void {
	writeFileSync(join(destDir, MANIFEST_NAME), `${JSON.stringify(manifest, null, "\t")}\n`);
}

describe("scripts/postinstall.js — Mach 12 seeding", () => {
	let workDir: string;
	let xdgHome: string;
	let fakeHome: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-postinstall-"));
		xdgHome = join(workDir, "xdg");
		fakeHome = join(workDir, "home");
		mkdirSync(fakeHome, { recursive: true });
	});

	afterEach(() => {
		rmSync(workDir, { recursive: true, force: true });
	});

	it("fresh seed: writes mach12/ with manifest into XDG_DATA_HOME/scramjet/", () => {
		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		const dest = join(xdgHome, "scramjet", "mach12");
		expect(existsSync(dest)).toBe(true);
		expect(existsSync(join(dest, "commands", "mach12:issue-create.md"))).toBe(true);
		expect(result.stdout).toContain("Seeded Mach 12 command set");

		// Manifest exists with current version and entries for every bundled file
		const manifest = readManifest(dest);
		expect(manifest.version).toBe(PKG_VERSION);
		expect(Object.keys(manifest.files).length).toBeGreaterThan(0);
		expect(manifest.files["commands/mach12:issue-create.md"]).toBeDefined();
		// Verify hash correctness
		const srcFile = join(REPO_ROOT, "mach12", "commands", "mach12:issue-create.md");
		expect(manifest.files["commands/mach12:issue-create.md"]).toBe(sha256(srcFile));
	});

	it("same-version manifest: exits fast without warnings or modifications", () => {
		// First run — fresh seed
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Edit a file — should be untouched by same-version run
		const editTarget = join(dest, "commands", "mach12:issue-create.md");
		writeFileSync(editTarget, "user edited content");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stdout).not.toContain("Seeded");
		expect(result.stdout).not.toContain("upgraded");
		expect(result.stderr).not.toContain("Preserved");
		// Edited file left alone
		expect(readFileSync(editTarget, "utf-8")).toBe("user edited content");
	});

	it("unedited upgrade: replaces bundled content when user has not edited", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Simulate an upgrade: bump the manifest version to an older one
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		writeManifest(dest, manifest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		// Manifest now has current version
		const updated = readManifest(dest);
		expect(updated.version).toBe(PKG_VERSION);
	});

	it("edited file preserved during upgrade with warning", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Edit a bundled file
		const editTarget = join(dest, "commands", "mach12:issue-create.md");
		writeFileSync(editTarget, "my custom command");

		// Simulate upgrade
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		writeManifest(dest, manifest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Preserved edited files");
		expect(result.stderr).toContain("mach12:issue-create.md");
		// Edited file NOT overwritten
		expect(readFileSync(editTarget, "utf-8")).toBe("my custom command");
		// Manifest updated
		expect(readManifest(dest).version).toBe(PKG_VERSION);
	});

	it("removed-from-bundle: unedited file deleted, edited file preserved", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Add fake entries to the manifest that don't exist in the current bundle
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		// Create files "in the old version"
		const uneditedRemoved = join(dest, "commands", "mach12:old-command.md");
		const editedRemoved = join(dest, "commands", "mach12:old-edited.md");
		writeFileSync(uneditedRemoved, "old content");
		writeFileSync(editedRemoved, "old content");
		manifest.files["commands/mach12:old-command.md"] = sha256(uneditedRemoved);
		manifest.files["commands/mach12:old-edited.md"] = sha256(editedRemoved);
		writeManifest(dest, manifest);
		// Now edit one of them
		writeFileSync(editedRemoved, "user edited old command");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		// Unedited removed file deleted
		expect(existsSync(uneditedRemoved)).toBe(false);
		// Edited removed file preserved
		expect(existsSync(editedRemoved)).toBe(true);
		expect(readFileSync(editedRemoved, "utf-8")).toBe("user edited old command");
		expect(result.stderr).toContain("removed from bundle but edited");
	});

	it("missing-on-disk bundled file is reseeded during upgrade", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Delete a bundled file
		const target = join(dest, "commands", "mach12:issue-create.md");
		rmSync(target);

		// Simulate upgrade
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		writeManifest(dest, manifest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		// File restored
		expect(existsSync(target)).toBe(true);
	});

	it("user-created file at a path colliding with new bundle entry is preserved", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Simulate: user creates a file, then a new version adds a bundled file at the same path.
		// Bump manifest version to trigger upgrade, but DON'T add the path to manifest.files
		// (it wasn't in the old version's bundle).
		const collidingFile = join(dest, "commands", "mach12:issue-create.md");
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		// Remove the colliding path from manifest so it looks like a new bundle addition
		delete manifest.files["commands/mach12:issue-create.md"];
		writeManifest(dest, manifest);
		// Overwrite with user content — now it exists on disk but not in old manifest
		writeFileSync(collidingFile, "user created this first");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("new in bundle but pre-existing on disk");
		expect(readFileSync(collidingFile, "utf-8")).toBe("user created this first");
	});

	it("user-added file survives manifest-based upgrade", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const dest = join(xdgHome, "scramjet", "mach12");

		// Add a user file not in the bundle
		const userFile = join(dest, "commands", "my-custom:command.md");
		writeFileSync(userFile, "user custom");

		// Simulate upgrade
		const manifest = readManifest(dest);
		manifest.version = "0.0.0-old";
		writeManifest(dest, manifest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(readFileSync(userFile, "utf-8")).toBe("user custom");
	});

	it("healthy symlink untouched: no manifest, no backup, no legacy migration", () => {
		const dest = join(xdgHome, "scramjet", "mach12");
		mkdirSync(dirname(dest), { recursive: true });
		// Create a target directory and symlink to it (simulating dev setup)
		const target = join(workDir, "symlink-target");
		mkdirSync(target, { recursive: true });
		writeFileSync(join(target, "test.md"), "dev content");
		symlinkSync(target, dest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		// No seeded message, no migration, no backup
		expect(result.stdout).not.toContain("Seeded");
		expect(result.stderr).not.toContain("migrated");
		expect(result.stderr).not.toContain("backup");
		// Symlink still healthy
		expect(lstatSync(dest).isSymbolicLink()).toBe(true);
		expect(readFileSync(join(dest, "test.md"), "utf-8")).toBe("dev content");
	});

	it("legacy tree (no manifest): backup created, fresh seed, user-added files recovered", () => {
		// Simulate a legacy install: copy mach12 without manifest
		const dest = join(xdgHome, "scramjet", "mach12");
		mkdirSync(dirname(dest), { recursive: true });
		cpSync(join(REPO_ROOT, "mach12"), dest, { recursive: true });
		// Add a user-created file
		const userFile = join(dest, "commands", "my-custom:command.md");
		writeFileSync(userFile, "user custom command");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Legacy Mach 12 install migrated");
		expect(result.stderr).toContain("pre-upgrade-");
		expect(result.stderr).toContain("User-added files were copied");

		// Dest now has manifest
		const manifest = readManifest(dest);
		expect(manifest.version).toBe(PKG_VERSION);

		// User-added file was recovered into the new tree
		expect(existsSync(userFile)).toBe(true);
		expect(readFileSync(userFile, "utf-8")).toBe("user custom command");

		// Backup exists and contains the full original tree
		const backupDir = readdirSync(join(xdgHome, "scramjet")).find((d) => d.startsWith("mach12.pre-upgrade-"));
		expect(backupDir).toBeDefined();
		expect(existsSync(join(xdgHome, "scramjet", backupDir!, "commands", "mach12:issue-create.md"))).toBe(true);
	});

	it("legacy migration: edited bundled files stay only in backup", () => {
		const dest = join(xdgHome, "scramjet", "mach12");
		mkdirSync(dirname(dest), { recursive: true });
		cpSync(join(REPO_ROOT, "mach12"), dest, { recursive: true });
		// Edit a bundled file
		writeFileSync(join(dest, "commands", "mach12:issue-create.md"), "user edited");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);

		// Dest has the fresh bundled version, not the edit
		const destContent = readFileSync(join(dest, "commands", "mach12:issue-create.md"), "utf-8");
		expect(destContent).not.toBe("user edited");

		// Edit is in the backup
		const backupDir = readdirSync(join(xdgHome, "scramjet")).find((d) => d.startsWith("mach12.pre-upgrade-"));
		expect(backupDir).toBeDefined();
		const backupContent = readFileSync(
			join(xdgHome, "scramjet", backupDir!, "commands", "mach12:issue-create.md"),
			"utf-8",
		);
		expect(backupContent).toBe("user edited");
	});

	it("XDG guard: relative XDG_DATA_HOME is rejected with a warning and no writes", () => {
		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: "relative/path", HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("is not absolute");
		expect(result.stdout).not.toContain("Seeded");
	});

	it("missing source: pkgRoot without a mach12/ sibling exits 0 with a warning", () => {
		const fakePkg = join(workDir, "fake-pkg");
		mkdirSync(join(fakePkg, "scripts"), { recursive: true });
		cpSync(REAL_SCRIPT, join(fakePkg, "scripts", "postinstall.js"));
		// The fake package also needs a package.json for the version read
		writeFileSync(join(fakePkg, "package.json"), JSON.stringify({ version: "0.0.0" }));

		const result = runScript(join(fakePkg, "scripts", "postinstall.js"), { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Bundled Mach 12 source missing");
		expect(existsSync(join(xdgHome, "scramjet", "mach12"))).toBe(false);
	});

	it("dangling symlink is removed and fresh seed proceeds", () => {
		const dest = join(xdgHome, "scramjet", "mach12");
		mkdirSync(dirname(dest), { recursive: true });
		symlinkSync("/nonexistent/target", dest);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing dangling symlink");
		expect(result.stdout).toContain("Seeded Mach 12 command set");
		expect(existsSync(join(dest, MANIFEST_NAME))).toBe(true);
	});
});

describe("scripts/postinstall.js — stale subagent extension cleanup", () => {
	let workDir: string;
	let xdgHome: string;
	let fakeHome: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-postinstall-subagent-"));
		xdgHome = join(workDir, "xdg");
		fakeHome = join(workDir, "home");
		mkdirSync(fakeHome, { recursive: true });
	});

	afterEach(() => {
		rmSync(workDir, { recursive: true, force: true });
	});

	it("removes a dangling symlink at agent/extensions/subagent", () => {
		const extDir = join(fakeHome, ".scramjet", "agent", "extensions");
		const extSubagent = join(extDir, "subagent");
		mkdirSync(extDir, { recursive: true });
		symlinkSync("/nonexistent/target", extSubagent);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(pathExists(extSubagent)).toBe(false);
	});

	it("removes a stale manual-install directory at agent/extensions/subagent", () => {
		const extSubagent = join(fakeHome, ".scramjet", "agent", "extensions", "subagent");
		mkdirSync(extSubagent, { recursive: true });
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "index.ts"), join(extSubagent, "index.ts"));
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "agents.ts"), join(extSubagent, "agents.ts"));

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(pathExists(extSubagent)).toBe(false);
	});

	it("preserves user-owned directories at agent/extensions/subagent", () => {
		const extSubagent = join(fakeHome, ".scramjet", "agent", "extensions", "subagent");
		mkdirSync(extSubagent, { recursive: true });
		writeFileSync(join(extSubagent, "index.ts"), "// custom extension");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Preserving");
		expect(pathExists(extSubagent)).toBe(true);
		expect(readFileSync(join(extSubagent, "index.ts"), "utf-8")).toBe("// custom extension");
	});

	it("cleans stale extensions even when mach12 already exists", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		const extDir = join(fakeHome, ".scramjet", "agent", "extensions");
		const extSubagent = join(extDir, "subagent");
		mkdirSync(extDir, { recursive: true });
		symlinkSync("/nonexistent/target", extSubagent);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(pathExists(extSubagent)).toBe(false);
	});

	it("cleans stale extensions before rejecting a relative XDG_DATA_HOME", () => {
		const extDir = join(fakeHome, ".scramjet", "agent", "extensions");
		const extSubagent = join(extDir, "subagent");
		mkdirSync(extDir, { recursive: true });
		symlinkSync("/nonexistent/target", extSubagent);

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: "relative/path", HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(result.stderr).toContain("is not absolute");
		expect(pathExists(extSubagent)).toBe(false);
	});

	it("cleans stale extensions even when the bundled source is missing", () => {
		const fakePkg = join(workDir, "fake-pkg");
		mkdirSync(join(fakePkg, "scripts"), { recursive: true });
		cpSync(REAL_SCRIPT, join(fakePkg, "scripts", "postinstall.js"));
		writeFileSync(join(fakePkg, "package.json"), JSON.stringify({ version: "0.0.0" }));
		const extDir = join(fakeHome, ".scramjet", "agent", "extensions");
		const extSubagent = join(extDir, "subagent");
		mkdirSync(extDir, { recursive: true });
		symlinkSync("/nonexistent/target", extSubagent);

		const result = runScript(join(fakePkg, "scripts", "postinstall.js"), { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(result.stderr).toContain("Bundled Mach 12 source missing");
		expect(pathExists(extSubagent)).toBe(false);
	});

	it("warns when stale extension cleanup fails for a non-ENOENT error", () => {
		rmSync(fakeHome, { recursive: true, force: true });
		writeFileSync(fakeHome, "not a directory");

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Stale extension check failed");
	});

	it("no-op when agent/extensions/subagent does not exist", () => {
		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome, HOME: fakeHome });
		expect(result.status).toBe(0);
		expect(result.stderr).not.toContain("Removing stale subagent extension");
		expect(result.stderr).not.toContain("Stale extension check failed");
	});
});
