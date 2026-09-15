import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
	existsSync,
	lstatSync,
	mkdirSync,
	mkdtempSync,
	readFileSync,
	readlinkSync,
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

interface RunResult {
	stdout: string;
	stderr: string;
	status: number;
}

function runScript(env: NodeJS.ProcessEnv): RunResult {
	const result = spawnSync(process.execPath, [REAL_SCRIPT], {
		env: { ...process.env, ...env },
		encoding: "utf-8",
	});
	return {
		stdout: result.stdout ?? "",
		stderr: result.stderr ?? "",
		status: result.status ?? -1,
	};
}

function pathExists(path: string): boolean {
	try {
		lstatSync(path);
		return true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
		throw error;
	}
}

function sha256(path: string): string {
	return createHash("sha256").update(readFileSync(path)).digest("hex");
}

describe("scripts/postinstall.js", () => {
	let workDir: string;
	let xdgHome: string;
	let fakeHome: string;
	let extensionDir: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-postinstall-"));
		xdgHome = join(workDir, "xdg");
		fakeHome = join(workDir, "home");
		extensionDir = join(fakeHome, ".scramjet", "agent", "extensions", "subagent");
		mkdirSync(fakeHome, { recursive: true });
	});

	afterEach(() => {
		rmSync(workDir, { recursive: true, force: true });
	});

	it("does not create bundled trees", () => {
		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stdout).toBe("");
		expect(pathExists(join(xdgHome, "scramjet", "mach12"))).toBe(false);
		expect(pathExists(join(xdgHome, "scramjet", "scramjet"))).toBe(false);
	});

	it("preserves legacy bundled trees byte-for-byte without following symlinks", () => {
		const root = join(xdgHome, "scramjet");
		const mach12 = join(root, "mach12");
		const scramjet = join(root, "scramjet");
		const outside = join(workDir, "outside.txt");
		mkdirSync(join(mach12, "commands"), { recursive: true });
		mkdirSync(scramjet, { recursive: true });
		writeFileSync(join(mach12, "commands", "mach12:legacy.md"), Buffer.from("legacy\0mach12"));
		writeFileSync(join(scramjet, ".seed-manifest.json"), '{"version":"0.43.5","files":{}}\n');
		writeFileSync(outside, "outside");
		symlinkSync(outside, join(mach12, "linked"));
		const before = {
			command: sha256(join(mach12, "commands", "mach12:legacy.md")),
			manifest: sha256(join(scramjet, ".seed-manifest.json")),
			outside: sha256(outside),
			link: readlinkSync(join(mach12, "linked")),
		};

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(sha256(join(mach12, "commands", "mach12:legacy.md"))).toBe(before.command);
		expect(sha256(join(scramjet, ".seed-manifest.json"))).toBe(before.manifest);
		expect(sha256(outside)).toBe(before.outside);
		expect(readlinkSync(join(mach12, "linked"))).toBe(before.link);
	});

	it("removes a whole-directory symlink only when it targets the deprecated example", () => {
		mkdirSync(dirname(extensionDir), { recursive: true });
		symlinkSync(OLD_SUBAGENT_EXAMPLE, extensionDir);

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(pathExists(extensionDir)).toBe(false);
	});

	it.each([
		["a foreign symlink", () => join(workDir, "foreign")],
		["a dangling symlink", () => join(workDir, "missing")],
	])("preserves %s", (_name, target) => {
		mkdirSync(dirname(extensionDir), { recursive: true });
		const targetPath = target();
		if (!targetPath.endsWith("missing")) mkdirSync(targetPath, { recursive: true });
		symlinkSync(targetPath, extensionDir);

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Preserving");
		expect(lstatSync(extensionDir).isSymbolicLink()).toBe(true);
		expect(readlinkSync(extensionDir)).toBe(targetPath);
	});

	it("removes a manual directory containing exactly the deprecated file symlinks", () => {
		mkdirSync(extensionDir, { recursive: true });
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "index.ts"), join(extensionDir, "index.ts"));
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "agents.ts"), join(extensionDir, "agents.ts"));

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Removing stale subagent extension");
		expect(pathExists(extensionDir)).toBe(false);
	});

	it("preserves every other manual directory", () => {
		mkdirSync(extensionDir, { recursive: true });
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "index.ts"), join(extensionDir, "index.ts"));
		symlinkSync(join(OLD_SUBAGENT_EXAMPLE, "agents.ts"), join(extensionDir, "agents.ts"));
		writeFileSync(join(extensionDir, "custom.ts"), "custom");

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Preserving");
		expect(readFileSync(join(extensionDir, "custom.ts"), "utf-8")).toBe("custom");
	});

	it("preserves a regular file at the extension path", () => {
		mkdirSync(dirname(extensionDir), { recursive: true });
		writeFileSync(extensionDir, "user content");

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Preserving");
		expect(readFileSync(extensionDir, "utf-8")).toBe("user content");
	});

	it("warns when cleanup inspection fails", () => {
		rmSync(fakeHome, { recursive: true });
		writeFileSync(fakeHome, "not a directory");

		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toContain("Stale extension check failed");
	});

	it("is quiet when the deprecated extension does not exist", () => {
		const result = runScript({ XDG_DATA_HOME: xdgHome, HOME: fakeHome });

		expect(result.status).toBe(0);
		expect(result.stderr).toBe("");
		expect(existsSync(extensionDir)).toBe(false);
	});
});
