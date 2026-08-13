import { mkdir, mkdtemp, readdir, readFile, readlink, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { afterEach, describe, expect, test } from "vitest";
import type { NpmRecoveryMetadata, SelfUpdateCommand } from "../src/config.js";
import { runNpmSelfUpdateTransaction } from "../src/npm-self-update-transaction.js";

const temporaryDirectories: string[] = [];

type Replacement =
	| "valid"
	| "fail"
	| "destructive-fail"
	| "invalid-name"
	| "invalid-bin"
	| "invalid-link"
	| "probe-term-responsive"
	| "probe-term-ignoring"
	| "probe-signaled"
	| "probe-spawn-error"
	| "probe-near-timeout"
	| "damage-backup";

interface Fixture {
	root: string;
	productRoot: string;
	launcherPath: string;
	probeLog: string;
	metadata: NpmRecoveryMetadata;
	command: SelfUpdateCommand;
}

async function writeProduct(productRoot: string, probeLog: string, oldProbeFails: boolean): Promise<void> {
	await mkdir(join(productRoot, "bin"), { recursive: true });
	await mkdir(join(productRoot, "node_modules", "@leanandmean", "coding-agent"), { recursive: true });
	await writeFile(
		join(productRoot, "package.json"),
		JSON.stringify({ name: "@leanandmean/scramjet", bin: { scramjet: "bin/scramjet.js" } }),
	);
	await writeFile(
		join(productRoot, "bin", "scramjet.js"),
		`#!/usr/bin/env node\nimport { appendFileSync } from "node:fs";\nif (process.argv[2] !== "--help") process.exit(9);\nappendFileSync(${JSON.stringify(probeLog)}, "old\\n");\nprocess.exit(${oldProbeFails ? 7 : 0});\n`,
		{ mode: 0o755 },
	);
}

async function createFixture(replacement: Replacement, oldProbeFails = false): Promise<Fixture> {
	const root = await mkdtemp(join(tmpdir(), "scramjet-transaction-"));
	temporaryDirectories.push(root);
	const productRoot = join(root, "lib", "node_modules", "@leanandmean", "scramjet");
	const launcherPath = join(root, "bin", "scramjet");
	const probeLog = join(root, "probe.log");
	await writeProduct(productRoot, probeLog, oldProbeFails);
	await mkdir(dirname(launcherPath), { recursive: true });
	const launcherLinkText = relative(dirname(launcherPath), join(productRoot, "bin", "scramjet.js"));
	await symlink(launcherLinkText, launcherPath);
	const scriptPath = join(root, "fake-npm.mjs");
	await writeFile(
		scriptPath,
		`import { chmod, mkdir, readFile, readdir, writeFile, symlink, rm } from "node:fs/promises";
import { dirname, join, relative } from "node:path";
const [productRoot, launcherPath, replacement, probeLog] = process.argv.slice(2);
if (replacement === "damage-backup") {
  const parent = dirname(productRoot);
  const backup = (await readdir(parent)).find((name) => name.includes("scramjet-backup"));
  const manifestPath = join(parent, backup, "package.json");
  const manifest = JSON.parse(await readFile(manifestPath, "utf8"));
  manifest.name = "@other/package";
  await writeFile(manifestPath, JSON.stringify(manifest));
  process.exit(23);
}
if (replacement === "fail") process.exit(23);
await mkdir(join(productRoot, "bin"), { recursive: true });
await mkdir(join(productRoot, "node_modules", "@leanandmean", "coding-agent"), { recursive: true });
const name = replacement === "invalid-name" ? "@other/package" : "@leanandmean/scramjet";
const bin = replacement === "invalid-bin" ? "bin/other.js" : "bin/scramjet.js";
await writeFile(join(productRoot, "package.json"), JSON.stringify({ name, bin: { scramjet: bin } }));
let launcher = "#!/usr/bin/env node\\nimport { appendFileSync } from 'node:fs';\\nif (process.argv[2] !== '--help') process.exit(9);\\nappendFileSync(" + JSON.stringify(probeLog) + ", 'new\\\\n');\\n";
if (replacement === "probe-term-responsive") launcher = "#!/usr/bin/env node\\nif (process.argv[2] !== '--help') process.exit(9); setInterval(() => {}, 1000);\\n";
if (replacement === "probe-term-ignoring") launcher = "#!/usr/bin/env node\\nif (process.argv[2] !== '--help') process.exit(9); process.on('SIGTERM', () => {}); setInterval(() => {}, 1000);\\n";
if (replacement === "probe-signaled") launcher = "#!/usr/bin/env node\\nif (process.argv[2] !== '--help') process.exit(9); process.kill(process.pid, 'SIGTERM');\\n";
if (replacement === "probe-near-timeout") launcher = "#!/usr/bin/env node\\nif (process.argv[2] !== '--help') process.exit(9); setTimeout(() => process.exit(0), 20);\\n";
await writeFile(join(productRoot, "bin", "scramjet.js"), launcher, { mode: 0o755 });
if (replacement === "probe-spawn-error") await chmod(join(productRoot, "bin", "scramjet.js"), 0o644);
await rm(launcherPath, { force: true });
const target = replacement === "invalid-link" ? join(productRoot, "bin", "missing.js") : join(productRoot, "bin", "scramjet.js");
await symlink(relative(dirname(launcherPath), target), launcherPath);
if (replacement === "destructive-fail") process.exit(23);
`,
	);
	const productStat = await stat(productRoot);
	const productParentStat = await stat(dirname(productRoot));
	const launcherParentStat = await stat(dirname(launcherPath));
	const metadata: NpmRecoveryMetadata = {
		packageName: "@leanandmean/scramjet",
		productRoot,
		packageRootType: "directory",
		runtimeRoot: join(productRoot, "node_modules", "@leanandmean", "coding-agent"),
		manifestPath: join(productRoot, "package.json"),
		declaredBinPath: "bin/scramjet.js",
		binTargetPath: join(productRoot, "bin", "scramjet.js"),
		launcherPath,
		launcherType: "symbolic-link",
		launcherLinkText,
		launcherTargetPath: join(productRoot, "bin", "scramjet.js"),
		productParentPath: dirname(productRoot),
		launcherParentPath: dirname(launcherPath),
		productDevice: productStat.dev,
		productParentDevice: productParentStat.dev,
		launcherParentDevice: launcherParentStat.dev,
		layout: "npm-posix-product-tree",
	};
	const command: SelfUpdateCommand = {
		command: process.execPath,
		args: [scriptPath, productRoot, launcherPath, replacement, probeLog],
		display: "fake npm update",
		npmRecovery: metadata,
	};
	return { root, productRoot, launcherPath, probeLog, metadata, command };
}

afterEach(async () => {
	await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })));
});

describe("runNpmSelfUpdateTransaction", () => {
	test("commits only after the replacement structure and launcher probe pass", async () => {
		const fixture = await createFixture("valid");

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata);

		expect(outcome).toMatchObject({ status: "committed", retainedPaths: [], cleanupFailures: [] });
		expect(await readlink(fixture.launcherPath)).toBe(fixture.metadata.launcherLinkText);
		expect(await readFile(fixture.probeLog, "utf8")).toBe("new\n");
	});

	test.each(["fail", "destructive-fail", "invalid-name", "invalid-bin", "invalid-link"] as const)(
		"restores and verifies the prior runtime after %s replacement",
		async (replacement) => {
			const fixture = await createFixture(replacement);

			const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata);

			expect(outcome.status).toBe("restored");
			expect(await readlink(fixture.launcherPath)).toBe(fixture.metadata.launcherLinkText);
			expect(await readFile(join(fixture.productRoot, "bin", "scramjet.js"), "utf8")).toContain("old");
			expect(await readFile(fixture.probeLog, "utf8")).toBe("old\n");
		},
	);

	test.each(["probe-term-responsive", "probe-term-ignoring", "probe-signaled", "probe-spawn-error"] as const)(
		"fully reaps a failed %s probe before restoring",
		async (replacement) => {
			const fixture = await createFixture(replacement);

			const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
				probeTimeoutMs: 2_000,
				probeTerminationGraceMs: 100,
			});

			expect(outcome.status).toBe("restored");
			expect(await readFile(join(fixture.productRoot, "bin", "scramjet.js"), "utf8")).toContain("old");
			expect(await readFile(fixture.probeLog, "utf8")).toBe("old\n");
		},
		15_000,
	);

	test("does not time out a probe that settles safely near the boundary", async () => {
		const fixture = await createFixture("probe-near-timeout");

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
			probeTimeoutMs: 500,
			probeTerminationGraceMs: 25,
		});

		expect(outcome.status).toBe("committed");
	});

	test("reports restoration as unverified when the preserved runtime was damaged", async () => {
		const fixture = await createFixture("damage-backup");

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata);

		expect(outcome.status).toBe("restoration-unverified");
		if (outcome.status === "restoration-unverified") {
			expect(outcome.restorationFailures[0]?.phase).toBe("restored-structure");
		}
	});

	test("reports restoration as unverified when the restored launcher probe fails", async () => {
		const fixture = await createFixture("invalid-name", true);

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata);

		expect(outcome.status).toBe("restoration-unverified");
		if (outcome.status === "restoration-unverified") {
			expect(outcome.restorationFailures[0]?.phase).toBe("restored-probe");
			expect(outcome.retainedPaths).toHaveLength(1);
		}
		expect(await readFile(fixture.probeLog, "utf8")).toBe("old\n");
	});

	test.each(["EBUSY", "EACCES", "EPERM", "EIO"])(
		"keeps a verified replacement when backup cleanup fails with %s",
		async (code) => {
			const fixture = await createFixture("valid");

			const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
				removeArtifact: async () => {
					throw Object.assign(new Error(code), { code });
				},
			});

			expect(outcome.status).toBe("committed-with-retained-artifacts");
			expect(outcome.retainedPaths).toHaveLength(1);
			expect(await readFile(join(fixture.productRoot, "bin", "scramjet.js"), "utf8")).toContain("new");
		},
	);

	test("keeps the verified restored runtime when quarantine cleanup fails", async () => {
		const fixture = await createFixture("invalid-name");

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
			removeArtifact: async () => {
				throw Object.assign(new Error("not empty"), { code: "ENOTEMPTY" });
			},
		});

		expect(outcome.status).toBe("restored");
		expect(outcome.retainedPaths).toHaveLength(1);
		expect(await readFile(join(fixture.productRoot, "bin", "scramjet.js"), "utf8")).toContain("old");
	});

	test("refuses to clean an artifact whose identity was substituted", async () => {
		const fixture = await createFixture("valid");

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
			beforeCleanup: async (path) => {
				await rm(path, { recursive: true });
				await mkdir(path);
			},
		});

		expect(outcome.status).toBe("committed-with-retained-artifacts");
		expect(outcome.retainedPaths).toHaveLength(1);
		if (outcome.status === "committed-with-retained-artifacts") {
			expect(outcome.cleanupFailures[0]?.error.message).toContain("identity changed");
		}
	});

	test("does not evacuate the product when a derived transaction path already exists", async () => {
		const fixture = await createFixture("valid");
		const staleBackup = join(dirname(fixture.productRoot), ".scramjet.scramjet-backup-fixed");
		await mkdir(staleBackup);

		const outcome = await runNpmSelfUpdateTransaction(fixture.command, fixture.metadata, {
			transactionId: "fixed",
		});

		expect(outcome).toMatchObject({ status: "restoration-unverified", updateFailure: { phase: "revalidation" } });
		expect(await readFile(join(fixture.productRoot, "bin", "scramjet.js"), "utf8")).toContain("old");
		expect(await readdir(staleBackup)).toEqual([]);
	});
});
