/**
 * Postinstall: bundled-tree seeding into XDG_DATA_HOME/scramjet/mach12.
 *
 * The most important property — "user customizations survive npm install" —
 * was previously asserted only in prose. We exercise the script in a
 * subprocess against a controlled XDG_DATA_HOME so each run is hermetic.
 * (F30, S4)
 *
 * scripts/postinstall.js resolves its source by computing pkgRoot from
 * import.meta.url (relative to the script file), so re-running the *real*
 * scripts/postinstall.js with a temp XDG_DATA_HOME is the closest thing to
 * a true integration test we can write without an actual `npm install`.
 */

import { spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..");
const REAL_SCRIPT = join(REPO_ROOT, "scripts", "postinstall.js");

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

describe("scripts/postinstall.js — Mach 12 seeding", () => {
	let workDir: string;
	let xdgHome: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-postinstall-"));
		xdgHome = join(workDir, "xdg");
	});

	afterEach(() => {
		rmSync(workDir, { recursive: true, force: true });
	});

	it("fresh seed: writes mach12/ into XDG_DATA_HOME/scramjet/ on first run", () => {
		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome });
		expect(result.status).toBe(0);
		const dest = join(xdgHome, "scramjet", "mach12");
		expect(existsSync(dest)).toBe(true);
		// A representative file from the bundled tree is present.
		expect(existsSync(join(dest, "commands", "mach12:issue-create.md"))).toBe(true);
		expect(result.stdout).toContain("Seeded Mach 12 command set");
	});

	it("idempotent: second run is a silent skip; existing tree untouched", () => {
		runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome });
		const dest = join(xdgHome, "scramjet", "mach12");
		// Inject a user customization the script must not clobber.
		const userFile = join(dest, "USER_EDIT.md");
		writeFileSync(userFile, "user content");
		const userMtime = statSync(userFile).mtimeMs;
		const destMtime = statSync(dest).mtimeMs;

		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: xdgHome });
		expect(result.status).toBe(0);
		// Silent skip: no "Seeded" log on the second run.
		expect(result.stdout).not.toContain("Seeded");
		// User edit survived.
		expect(readFileSync(userFile, "utf-8")).toBe("user content");
		expect(statSync(userFile).mtimeMs).toBe(userMtime);
		expect(statSync(dest).mtimeMs).toBe(destMtime);
	});

	it("XDG guard: relative XDG_DATA_HOME is rejected with a warning and no writes", () => {
		const result = runScript(REAL_SCRIPT, { XDG_DATA_HOME: "relative/path" });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("is not absolute");
		// Nothing should have been written anywhere we can check; the cwd is the
		// repo, and the script only writes to XDG_DATA_HOME — confirm no
		// "Seeded" log fired.
		expect(result.stdout).not.toContain("Seeded");
	});

	it("missing source: pkgRoot without a mach12/ sibling exits 0 with a warning", () => {
		// Reproduce the script in a fake package layout so pkgRoot computes to
		// a directory that lacks mach12/. The script reads import.meta.url to
		// derive pkgRoot = dirname(scriptDir), so placing the copied script at
		// <fakePkg>/scripts/postinstall.js gives pkgRoot = <fakePkg>.
		const fakePkg = join(workDir, "fake-pkg");
		mkdirSync(join(fakePkg, "scripts"), { recursive: true });
		cpSync(REAL_SCRIPT, join(fakePkg, "scripts", "postinstall.js"));

		const result = runScript(join(fakePkg, "scripts", "postinstall.js"), { XDG_DATA_HOME: xdgHome });
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Bundled Mach 12 source missing");
		expect(existsSync(join(xdgHome, "scramjet", "mach12"))).toBe(false);
	});
});
