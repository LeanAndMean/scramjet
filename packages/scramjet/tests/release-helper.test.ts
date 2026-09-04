import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	compareVersions,
	isTransientReadError,
	loadInventory,
	pollRead,
	publishPackage,
	run,
	validateIdentity,
} from "../../../.github/scripts/release.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "../../..");
const HELPER = join(REPO_ROOT, ".github", "scripts", "release.mjs");
const SHA = execFileSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8" }).trim();
const INVENTORY = loadInventory(REPO_ROOT);
const SCRAMJET_VERSION = INVENTORY.find(({ name }) => name === "@leanandmean/scramjet")!.version;
const RELEASE_ENV = {
	GITHUB_EVENT_NAME: "push",
	GITHUB_REF: `refs/tags/v${SCRAMJET_VERSION}`,
	GITHUB_SHA: SHA,
	GITHUB_WORKFLOW_REF: `LeanAndMean/scramjet/.github/workflows/release.yml@refs/tags/v${SCRAMJET_VERSION}`,
	GITHUB_RUN_ATTEMPT: "1",
};
const RELEASE_METADATA_PATHS = [...INVENTORY.map(({ workspace }) => `${workspace}/package.json`), "package-lock.json"];

function copyReleaseMetadata(root: string) {
	for (const path of RELEASE_METADATA_PATHS) {
		mkdirSync(dirname(join(root, path)), { recursive: true });
		writeFileSync(join(root, path), readFileSync(join(REPO_ROOT, path)));
	}
}

function mutateJson(root: string, path: string, mutate: (value: any) => void) {
	const file = join(root, path);
	const value = JSON.parse(readFileSync(file, "utf8"));
	mutate(value);
	writeFileSync(file, JSON.stringify(value));
}

interface FakeState {
	packages: Record<string, { versions: string[]; distTags: Record<string, string> }>;
	targets: Record<string, { name: string; version: string }>;
	calls: string[][];
	failure?: { name: string; field: string; output?: string; stderrOutput?: string; status?: number };
	failureAfterPublish?: {
		name: string;
		field: string;
		output?: string;
		stderrOutput?: string;
		status?: number;
		remaining?: number;
	};
	publishFailure?: string;
	race?: string;
	unexpectedTagChange?: string;
	versionQueries?: Record<string, number>;
	publicationCounts?: Record<string, number>;
	prePublishLatest?: Record<string, string>;
	visibilityDelays?: Record<string, number>;
	tagVisibilityDelays?: Record<string, number>;
	attestationDelays?: Record<string, number>;
	missingAttestation?: string;
	wrongAttestationPredicate?: string;
	installFailure?: boolean;
	auditFailure?: boolean;
	cliFailure?: boolean;
	installedVersionOverrides?: Record<string, string>;
	verificationPaths?: { project: string; cache: string; home: string; xdg: string };
}

const FAKE_NPM = `#!/usr/bin/env node
const fs = require("node:fs");
const { createHash } = require("node:crypto");
const statePath = process.env.FAKE_NPM_STATE;
const state = JSON.parse(fs.readFileSync(statePath, "utf8"));
const args = process.argv.slice(2);
state.calls.push(args);
function save() { fs.writeFileSync(statePath, JSON.stringify(state)); }
function stop(message) { save(); console.error(message); process.exit(1); }
if (args[0] === "view") {
  const spec = args[1];
  const versionSeparator = spec.lastIndexOf("@");
  const name = versionSeparator > 0 ? spec.slice(0, versionSeparator) : spec;
  const field = args[2];
  if (state.failure?.name === name && state.failure?.field === field) {
    save();
    if (state.failure.status) { process.stdout.write(state.failure.output ?? ""); console.error(state.failure.stderrOutput ?? "npm registry request failed"); process.exit(state.failure.status); }
    process.stdout.write(state.failure.output ?? "not json"); process.exit(0);
  }
  const postPublishFailure = state.failureAfterPublish;
  if ((state.publicationCounts?.[name] ?? 0) > 0 && postPublishFailure?.name === name && postPublishFailure?.field === field && (postPublishFailure.remaining ?? 1) > 0) {
    if (postPublishFailure.remaining !== undefined) postPublishFailure.remaining -= 1;
    save();
    if (postPublishFailure.status) { process.stdout.write(postPublishFailure.output ?? ""); console.error(postPublishFailure.stderrOutput ?? "npm registry request failed"); process.exit(postPublishFailure.status); }
    process.stdout.write(postPublishFailure.output ?? "not json"); process.exit(0);
  }
  const pkg = state.packages[name];
  if (!pkg) stop("unknown package");
  if (field === "dist.attestations.url" || field === "dist.attestations") {
    const version = spec.slice(versionSeparator + 1);
    const target = Object.values(state.targets).find((entry) => entry.name === name);
    if (versionSeparator <= 0 || target?.version !== version || !pkg.versions.includes(version)) stop("unknown package version");
    if ((state.attestationDelays?.[name] ?? 0) > 0) {
      state.attestationDelays[name] -= 1;
      save(); process.exit(0);
    }
    const url = "https://registry.npmjs.org/fake/" + encodeURIComponent(name) + "/" + version;
    if (field === "dist.attestations.url") {
      save(); process.stdout.write(JSON.stringify(url)); process.exit(0);
    }
    const attestations = state.missingAttestation === name ? {} : {
      url,
      provenance: { predicateType: state.wrongAttestationPredicate === name ? "https://example.test/predicate" : "https://slsa.dev/provenance/v1" },
    };
    save(); process.stdout.write(JSON.stringify(attestations)); process.exit(0);
  }
  if (field === "versions") {
    state.versionQueries ??= {};
    state.versionQueries[name] = (state.versionQueries[name] ?? 0) + 1;
    const target = Object.values(state.targets).find((entry) => entry.name === name);
    if (state.race === name && state.versionQueries[name] === 2) pkg.versions.push(target.version);
    if ((state.publicationCounts?.[name] ?? 0) > 0 && (state.visibilityDelays?.[name] ?? 0) > 0) {
      state.visibilityDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify(pkg.versions.filter((version) => version !== target.version))); process.exit(0);
    }
    save(); process.stdout.write(JSON.stringify(pkg.versions)); process.exit(0);
  }
  if (field === "dist-tags") {
    if ((state.publicationCounts?.[name] ?? 0) > 0 && (state.tagVisibilityDelays?.[name] ?? 0) > 0) {
      state.tagVisibilityDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify({ ...pkg.distTags, latest: state.prePublishLatest[name] })); process.exit(0);
    }
    save(); process.stdout.write(JSON.stringify(pkg.distTags)); process.exit(0);
  }
  stop("unexpected view");
}
if (args[0] === "publish") {
  const workspace = args[args.indexOf("-w") + 1];
  const target = state.targets[workspace];
  if (!target) stop("unknown workspace");
  if (state.publishFailure === target.name) stop("publish failed");
  const pkg = state.packages[target.name];
  state.publicationCounts ??= {};
  state.prePublishLatest ??= {};
  state.publicationCounts[target.name] = (state.publicationCounts[target.name] ?? 0) + 1;
  state.prePublishLatest[target.name] = pkg.distTags.latest;
  pkg.versions.push(target.version);
  pkg.distTags.latest = target.version;
  if (state.unexpectedTagChange === target.name) pkg.distTags.scramjet = target.version;
  save(); process.stdout.write("published"); process.exit(0);
}
if (args[0] === "install") {
  state.verificationPaths = {
    project: process.cwd(),
    cache: process.env.npm_config_cache,
    home: process.env.HOME,
    xdg: process.env.XDG_DATA_HOME,
  };
  if (state.installFailure) stop("install failed");
  for (const target of Object.values(state.targets)) {
    const packageDir = require("node:path").join(process.cwd(), "node_modules", target.name);
    fs.mkdirSync(packageDir, { recursive: true });
    fs.writeFileSync(require("node:path").join(packageDir, "package.json"), JSON.stringify({
      name: target.name,
      version: state.installedVersionOverrides?.[target.name] ?? target.version,
    }));
  }
  const binDir = require("node:path").join(process.cwd(), "node_modules", ".bin");
  fs.mkdirSync(binDir, { recursive: true });
  fs.writeFileSync(require("node:path").join(binDir, "scramjet"), [
    "#!/usr/bin/env node",
    'const fs = require("node:fs");',
    'const state = JSON.parse(fs.readFileSync(process.env.FAKE_NPM_STATE, "utf8"));',
    'state.calls.push(["installed-scramjet", ...process.argv.slice(2)]);',
    'fs.writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));',
    'if (state.cliFailure) process.exit(1);',
    'process.stdout.write("scramjet help");',
  ].join("\\n"));
  fs.chmodSync(require("node:path").join(binDir, "scramjet"), 0o755);
  save(); process.exit(0);
}
if (args[0] === "audit" && args[1] === "signatures") {
  if (state.auditFailure) stop("audit failed");
  save(); process.exit(0);
}
stop("unexpected command");
`;

function previousVersion(version: string): string {
	const runtime = /^(\d+\.\d+\.\d+-scramjet\.)(\d+)$/.exec(version);
	if (runtime) return `${runtime[1]}${Number(runtime[2]) - 1}`;
	const stable = /^(\d+)\.(\d+)\.(\d+)$/.exec(version)!;
	const patch = Number(stable[3]);
	if (patch > 0) return `${stable[1]}.${stable[2]}.${patch - 1}`;
	return `${stable[1]}.${Number(stable[2]) - 1}.0`;
}

function initialState(): FakeState {
	return {
		packages: Object.fromEntries(
			INVENTORY.map(({ name, version }) => {
				const previous = previousVersion(version);
				return [name, { versions: [previous], distTags: { latest: previous, scramjet: "preserved" } }];
			}),
		),
		targets: Object.fromEntries(INVENTORY.map(({ workspace, name, version }) => [workspace, { name, version }])),
		calls: [],
	};
}

function runHelper(mode: string, statePath: string, args = mode === "preflight" ? [SHA] : []) {
	const script = ["publish", "registry-preflight", "verify"].includes(mode)
		? join(dirname(statePath), "runner.mjs")
		: HELPER;
	return spawnSync(process.execPath, [script, mode, ...args], {
		cwd: REPO_ROOT,
		encoding: "utf8",
		env: {
			...process.env,
			...RELEASE_ENV,
			PATH: `${dirname(statePath)}:${process.env.PATH}`,
			FAKE_NPM_STATE: statePath,
		},
	});
}

function readState(path: string): FakeState {
	return JSON.parse(readFileSync(path, "utf8"));
}

function publishCalls(state: FakeState): string[][] {
	return state.calls.filter(([command]) => command === "publish");
}

describe("release helper package and event validation", () => {
	it("loads the exact ordered package inventory with canonical repository metadata", () => {
		expect(INVENTORY.map(({ workspace, name }) => [workspace, name])).toEqual([
			["packages/tui", "@leanandmean/tui"],
			["packages/ai", "@leanandmean/ai"],
			["packages/agent", "@leanandmean/agent"],
			["packages/coding-agent", "@leanandmean/coding-agent"],
			["packages/scramjet", "@leanandmean/scramjet"],
		]);
	});

	it.each([
		[
			"package identity",
			"packages/agent",
			(manifest: Record<string, any>) => {
				manifest.name = "@leanandmean/ai";
			},
		],
		[
			"repository metadata",
			"packages/agent",
			(manifest: Record<string, any>) => {
				manifest.repository = "github:LeanAndMean/scramjet";
			},
		],
		[
			"publish registry",
			"packages/agent",
			(manifest: Record<string, any>) => {
				manifest.publishConfig = { access: "public", registry: "https://example.test/" };
			},
		],
		[
			"scoped publish registry",
			"packages/agent",
			(manifest: Record<string, any>) => {
				manifest.publishConfig = { access: "public", "@leanandmean:registry": "https://example.test/" };
			},
		],
		[
			"Scramjet prerelease version",
			"packages/scramjet",
			(manifest: Record<string, any>) => {
				manifest.version = "1.2.3-scramjet.1";
			},
		],
		...["1.2.3", "1.2.3-beta.1", "1.2.3-scramjet", "1.2.3-scramjet.x"].map((version) => [
			`runtime version ${version}`,
			"packages/tui",
			(manifest: Record<string, any>) => {
				manifest.version = version;
			},
		]),
		...["01.2.3", "1.02.3", "1.2.03"].map((version) => [
			`noncanonical Scramjet version ${version}`,
			"packages/scramjet",
			(manifest: Record<string, any>) => {
				manifest.version = version;
			},
		]),
		...["01.2.3-scramjet.1", "1.02.3-scramjet.1", "1.2.03-scramjet.1", "1.2.3-scramjet.01"].map((version) => [
			`noncanonical runtime version ${version}`,
			"packages/tui",
			(manifest: Record<string, any>) => {
				manifest.version = version;
			},
		]),
		...[
			["packages/agent", ["@leanandmean/ai"]],
			["packages/coding-agent", ["@leanandmean/agent", "@leanandmean/ai", "@leanandmean/tui"]],
			["packages/scramjet", INVENTORY.slice(0, -1).map(({ name }) => name)],
		].flatMap(([workspace, dependencyNames]) =>
			(dependencyNames as string[]).flatMap((name) =>
				([undefined, "^0.0.0", "0.0.0"] as Array<string | undefined>).map((value) => [
					`${value === undefined ? "missing" : value.startsWith("^") ? "ranged" : "mismatched"} ${name} dependency in ${workspace}`,
					workspace,
					(manifest: Record<string, any>) => {
						if (value === undefined) delete manifest.dependencies[name];
						else manifest.dependencies[name] = value;
					},
				]),
			),
		),
	])("rejects incorrect %s", (_label, targetWorkspace, mutate) => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-manifests-"));
		try {
			writeFileSync(join(root, "package-lock.json"), readFileSync(join(REPO_ROOT, "package-lock.json")));
			for (const { workspace } of INVENTORY) {
				mkdirSync(join(root, workspace), { recursive: true });
				const manifest = JSON.parse(readFileSync(join(REPO_ROOT, workspace, "package.json"), "utf8"));
				if (workspace === targetWorkspace) mutate(manifest);
				writeFileSync(join(root, workspace, "package.json"), JSON.stringify(manifest));
			}
			expect(() => loadInventory(root)).toThrow();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it.each([
		[
			"lockfile version",
			(lock: any) => {
				lock.lockfileVersion = 2;
			},
		],
		[
			"missing workspace",
			(lock: any) => {
				delete lock.packages["packages/ai"];
			},
		],
		[
			"extra workspace",
			(lock: any) => {
				lock.packages["packages/extra"] = { name: "extra", version: "1.0.0" };
			},
		],
		[
			"workspace name",
			(lock: any) => {
				lock.packages["packages/ai"].name = "@leanandmean/other";
			},
		],
		[
			"workspace version",
			(lock: any) => {
				lock.packages["packages/ai"].version = "0.0.0";
			},
		],
		[
			"missing lock dependency",
			(lock: any) => {
				delete lock.packages["packages/agent"].dependencies["@leanandmean/ai"];
			},
		],
		[
			"ranged lock dependency",
			(lock: any) => {
				lock.packages["packages/agent"].dependencies["@leanandmean/ai"] = "^0.0.0";
			},
		],
		[
			"extra lock dependency",
			(lock: any) => {
				lock.packages["packages/ai"].dependencies["@leanandmean/tui"] = "0.0.0";
			},
		],
		[
			"missing workspace link",
			(lock: any) => {
				delete lock.packages["node_modules/@leanandmean/ai"];
			},
		],
		[
			"extra workspace link",
			(lock: any) => {
				lock.packages["node_modules/@leanandmean/extra"] = { resolved: "packages/extra", link: true };
			},
		],
		[
			"redirected workspace link",
			(lock: any) => {
				lock.packages["node_modules/@leanandmean/ai"].resolved = "packages/tui";
			},
		],
		[
			"non-link workspace",
			(lock: any) => {
				lock.packages["node_modules/@leanandmean/ai"].link = false;
			},
		],
	])("rejects incorrect %s", (_label, mutate) => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-lock-"));
		try {
			copyReleaseMetadata(root);
			mutateJson(root, "package-lock.json", mutate);
			expect(() => loadInventory(root)).toThrow();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it.each([
		[
			"an extra manifest dependency",
			"packages/ai/package.json",
			(manifest: any) => {
				manifest.dependencies["@leanandmean/tui"] = "0.0.0";
			},
		],
		[
			"a misplaced manifest dependency",
			"packages/agent/package.json",
			(manifest: any) => {
				manifest.devDependencies = { "@leanandmean/ai": manifest.dependencies["@leanandmean/ai"] };
			},
		],
	])("rejects %s", (_label, path, mutate) => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-closure-"));
		try {
			copyReleaseMetadata(root);
			mutateJson(root, path, mutate);
			expect(() => loadInventory(root)).toThrow();
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("accepts only an aligned push event, tag, workflow ref, SHA, attempt, and HEAD", () => {
		expect(validateIdentity(INVENTORY, RELEASE_ENV, () => SHA)).toEqual({ ref: RELEASE_ENV.GITHUB_REF, sha: SHA });
	});

	it("validates a clean checkout before dependencies are installed", () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-validate-"));
		try {
			mkdirSync(join(root, ".github", "scripts"), { recursive: true });
			writeFileSync(join(root, ".github", "scripts", "release.mjs"), readFileSync(HELPER));
			copyReleaseMetadata(root);
			const bin = join(root, "bin");
			mkdirSync(bin);
			writeFileSync(join(bin, "git"), `#!/bin/sh\nprintf '%s\\n' '${SHA}'\n`);
			chmodSync(join(bin, "git"), 0o755);
			const result = spawnSync(process.execPath, [join(root, ".github", "scripts", "release.mjs"), "validate"], {
				cwd: root,
				encoding: "utf8",
				env: { ...process.env, ...RELEASE_ENV, PATH: `${bin}:${process.env.PATH}` },
			});
			expect(result.stderr).toBe("");
			expect(result.status).toBe(0);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it.each([
		["manual event", { GITHUB_EVENT_NAME: "workflow_dispatch" }],
		["branch ref", { GITHUB_REF: "refs/heads/main" }],
		["unprefixed tag", { GITHUB_REF: `refs/tags/${SCRAMJET_VERSION}` }],
		[
			"different workflow",
			{ GITHUB_WORKFLOW_REF: `LeanAndMean/scramjet/.github/workflows/other.yml@${RELEASE_ENV.GITHUB_REF}` },
		],
		[
			"different workflow ref",
			{ GITHUB_WORKFLOW_REF: "LeanAndMean/scramjet/.github/workflows/release.yml@refs/heads/main" },
		],
		["invalid SHA", { GITHUB_SHA: "not-a-sha" }],
		["missing run attempt", { GITHUB_RUN_ATTEMPT: undefined }],
		["a repeated run attempt", { GITHUB_RUN_ATTEMPT: "2" }],
	])("rejects %s", (_label, override) => {
		expect(() => validateIdentity(INVENTORY, { ...RELEASE_ENV, ...override }, () => SHA)).toThrow();
	});

	it("rejects a checkout that does not match the event SHA", () => {
		expect(() => validateIdentity(INVENTORY, RELEASE_ENV, () => "0".repeat(40))).toThrow(/HEAD/);
	});
});

describe("release helper version policy", () => {
	it("orders stable and Scramjet runtime versions without numeric precision loss", () => {
		expect(compareVersions("1.2.3", "1.2.2")).toBe(1);
		expect(compareVersions("0.74.1-scramjet.18", "0.74.1-scramjet.17")).toBe(1);
		expect(compareVersions("0.74.1", "0.74.1-scramjet.18")).toBe(1);
		expect(compareVersions("9007199254740993.0.0", "9007199254740992.0.0")).toBe(1);
		expect(compareVersions("1.0.0-scramjet.9007199254740993", "1.0.0-scramjet.9007199254740992")).toBe(1);
	});

	it.each(["1.2", "1.2.3-beta.1", "v1.2.3", "0.74.1-scramjet.x", "01.2.3", "1.2.3-scramjet.01"])(
		"rejects unknown version form %s",
		(version) => {
			expect(() => compareVersions(version, "1.2.3")).toThrow(/unsupported version/);
		},
	);
});

describe("release helper registry preflight and publication", () => {
	let workDir: string;
	let statePath: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-release-"));
		writeFileSync(join(workDir, "npm"), FAKE_NPM);
		chmodSync(join(workDir, "npm"), 0o755);
		writeFileSync(
			join(workDir, "runner.mjs"),
			`import { loadInventory, preflight, publish, validateIdentity, verify } from ${JSON.stringify(new URL("../../../.github/scripts/release.mjs", import.meta.url))};
try {
  const inventory = loadInventory();
  validateIdentity(inventory);
  if (process.argv[2] === "publish") await publish(inventory, { pollDependencies: { delayMs: 0, sleep: async () => {} } });
  else if (process.argv[2] === "verify") await verify(inventory, { pollDependencies: { delayMs: 0, sleep: async () => {} } });
  else preflight(inventory);
} catch (error) {
  console.error("release: " + error.message);
  process.exitCode = 1;
}
`,
		);
		statePath = join(workDir, "state.json");
		writeFileSync(statePath, JSON.stringify(initialState()));
	});

	afterEach(() => rmSync(workDir, { recursive: true, force: true }));

	it("registry-preflights all five packages without publishing", () => {
		const result = runHelper("registry-preflight", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout.match(/: missing;/g)).toHaveLength(5);
		const state = readState(statePath);
		expect(state.calls).toHaveLength(10);
		expect(publishCalls(state)).toHaveLength(0);
	});

	it.each([
		["a missing SHA", []],
		["an extra argument", [SHA, "extra"]],
		["a noncanonical SHA", [SHA.toUpperCase()]],
		["a nonexistent SHA", ["0".repeat(40)]],
	])("rejects preflight with %s before registry access", (_label, args) => {
		const result = runHelper("preflight", statePath, args);
		expect(result.status).not.toBe(0);
		expect(readState(statePath).calls).toHaveLength(0);
	});

	it("rejects an existing commit other than HEAD before registry access", () => {
		const otherSha = "1".repeat(40);
		writeFileSync(
			join(workDir, "git"),
			`#!/bin/sh
if [ "$1 $2" = "cat-file -e" ]; then exit 0; fi
if [ "$1 $2" = "rev-parse HEAD" ]; then printf '%s\\n' '${SHA}'; exit 0; fi
exit 1
`,
		);
		chmodSync(join(workDir, "git"), 0o755);
		const result = runHelper("preflight", statePath, [otherSha]);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("confirmed SHA must equal checked-out HEAD");
		expect(readState(statePath).calls).toHaveLength(0);
	});

	it("does not expose retained-package reconciliation", () => {
		const result = runHelper("reconcile", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("usage: release.mjs <validate|publish|verify>");
		expect(readState(statePath).calls).toHaveLength(0);
	});

	it.each([
		["unstaged manifest changes", "packages/agent/package.json", false],
		["staged manifest changes", "packages/agent/package.json", true],
		["unstaged lockfile changes", "package-lock.json", false],
		["staged lockfile changes", "package-lock.json", true],
	])("rejects %s before registry access", (_label, path, staged) => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-preflight-"));
		try {
			mkdirSync(join(root, ".github", "scripts"), { recursive: true });
			writeFileSync(join(root, ".github", "scripts", "release.mjs"), readFileSync(HELPER));
			copyReleaseMetadata(root);
			writeFileSync(join(root, "npm"), FAKE_NPM);
			chmodSync(join(root, "npm"), 0o755);
			const git = (args: string[]) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
			git(["init", "--quiet"]);
			git(["config", "user.name", "Release Test"]);
			git(["config", "user.email", "release-test@example.test"]);
			git(["add", "."]);
			git(["commit", "--quiet", "-m", "release fixture"]);
			const head = git(["rev-parse", "HEAD"]);
			mutateJson(root, path, (value) => {
				value.releaseFixtureDirty = true;
			});
			if (staged) git(["add", path]);
			const fixtureState = join(root, "state.json");
			writeFileSync(fixtureState, JSON.stringify(initialState()));
			const result = spawnSync(
				process.execPath,
				[join(root, ".github", "scripts", "release.mjs"), "preflight", head],
				{
					cwd: root,
					encoding: "utf8",
					env: {
						...process.env,
						PATH: `${root}:${process.env.PATH}`,
						FAKE_NPM_STATE: fixtureState,
						GIT_DIR: join(root, ".git"),
						GIT_WORK_TREE: root,
					},
				},
			);
			expect(result.status).not.toBe(0);
			expect(readState(fixtureState).calls).toHaveLength(0);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("publishes all missing packages in dependency order with explicit latest and provenance", () => {
		const result = runHelper("publish", statePath);
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const state = readState(statePath);
		const calls = publishCalls(state);
		expect(calls.map((args) => args[args.indexOf("-w") + 1])).toEqual(INVENTORY.map(({ workspace }) => workspace));
		for (const args of calls) {
			expect(args).toEqual([
				"publish",
				"-w",
				expect.any(String),
				"--access",
				"public",
				"--provenance",
				"--tag",
				"latest",
				"--registry",
				"https://registry.npmjs.org/",
			]);
		}
		for (const { name, version } of INVENTORY) {
			expect(state.packages[name].versions).toContain(version);
			expect(state.packages[name].distTags).toEqual({ latest: version, scramjet: "preserved" });
		}
	});

	it.each(["registry-preflight", "publish"])("rejects a present target before publication in %s", (mode) => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version);
		state.packages[present.name].distTags.latest = present.version;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper(mode, statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${present.name}@${present.version} already exists`);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("rejects a present target even when a newer latest exists", () => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version, "999.0.0");
		state.packages[present.name].distTags.latest = "999.0.0";
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${present.name}@${present.version} already exists`);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it.each([
		["failed lookup", { name: "@leanandmean/agent", field: "versions", status: 1 }],
		["malformed versions", { name: "@leanandmean/agent", field: "versions", output: "not-json" }],
		["malformed dist-tags", { name: "@leanandmean/agent", field: "dist-tags", output: "[]" }],
	])("aborts %s before any publication", (_label, failure) => {
		const state = initialState();
		state.failure = failure;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("rejects a target that would regress latest before any publication", () => {
		const state = initialState();
		const first = INVENTORY[0];
		state.packages[first.name].distTags.latest = "999.0.0";
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("not newer than latest");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("stops when a target appears between preflight and publication", () => {
		const state = initialState();
		state.race = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("appeared after preflight");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("reports forward-only recovery when a later target appears after publication began", () => {
		const state = initialState();
		state.race = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${INVENTORY[1].name}@${INVENTORY[1].version} appeared after preflight`);
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(publishCalls(readState(statePath)).map((args) => args[args.indexOf("-w") + 1])).toEqual([
			INVENTORY[0].workspace,
		]);
	});

	it("treats every publish failure as ambiguous without retrying", () => {
		const state = initialState();
		state.publishFailure = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(result.stderr).toContain("publish failed");
		const calls = publishCalls(readState(statePath));
		expect(calls.map((args) => args[args.indexOf("-w") + 1])).toEqual(
			INVENTORY.slice(0, 2).map(({ workspace }) => workspace),
		);
	});

	it("fails if publication changes a non-latest dist-tag", () => {
		const state = initialState();
		state.unexpectedTagChange = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("non-latest dist-tags changed");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("polls multi-minute version and attestation-metadata visibility through publication", () => {
		const state = initialState();
		const first = INVENTORY[0].name;
		state.visibilityDelays = { [first]: 18 };
		state.attestationDelays = { [first]: 18 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} registry visibility not ready`);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} attestation metadata not ready`);
		const finalState = readState(statePath);
		expect(finalState.publicationCounts?.[first]).toBe(1);
		expect(publishCalls(finalState)).toHaveLength(INVENTORY.length);
	});

	it("polls multi-minute latest-tag visibility through publication", () => {
		const state = initialState();
		const first = INVENTORY[0].name;
		state.tagVisibilityDelays = { [first]: 18 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} registry visibility not ready`);
		const finalState = readState(statePath);
		expect(finalState.publicationCounts?.[first]).toBe(1);
		expect(publishCalls(finalState)).toHaveLength(INVENTORY.length);
	}, 10_000);

	it.each([
		["malformed JSON", "versions", "not-json"],
		["malformed versions", "versions", "{}"],
		["malformed dist-tags", "dist-tags", "[]"],
	])("fails permanent post-publish visibility errors once: %s", (_label, field, output) => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = { name: first.name, field, output };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("publication state is ambiguous");
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === first.name && args[2] === field,
		);
		expect(calls).toHaveLength(3);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("fails malformed attestation metadata without retrying", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = { name: first.name, field: "dist.attestations.url", output: "{}" };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("attestation URL must be a string");
		const calls = readState(statePath).calls.filter(
			(args) =>
				args[0] === "view" && args[1] === `${first.name}@${first.version}` && args[2] === "dist.attestations.url",
		);
		expect(calls).toHaveLength(1);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("retries a structured transient npm registry failure after publication", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = {
			name: first.name,
			field: "versions",
			status: 1,
			output: JSON.stringify({ error: { code: "E429", summary: "Too Many Requests" } }),
			remaining: 1,
		};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first.name}@${first.version} registry visibility not ready`);
		expect(publishCalls(readState(statePath))).toHaveLength(INVENTORY.length);
	});

	it("falls back to a structured transient npm error on stderr", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = {
			name: first.name,
			field: "versions",
			status: 1,
			output: "not json",
			stderrOutput: JSON.stringify({ error: { code: "E429", summary: "Too Many Requests" } }),
			remaining: 1,
		};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first.name}@${first.version} registry visibility not ready`);
		expect(publishCalls(readState(statePath))).toHaveLength(INVENTORY.length);
	});

	it("uses a validated stdout error before a conflicting stderr error", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = {
			name: first.name,
			field: "versions",
			status: 1,
			output: JSON.stringify({ error: { code: "E401", summary: "Unauthorized" } }),
			stderrOutput: JSON.stringify({ error: { code: "E429", summary: "Too Many Requests" } }),
		};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === first.name && args[2] === "versions",
		);
		expect(calls).toHaveLength(3);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("fails an unrecognized npm registry error once after publication", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = {
			name: first.name,
			field: "versions",
			status: 1,
			output: JSON.stringify({ error: { code: "E401", summary: "Unauthorized" } }),
		};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("publication state is ambiguous");
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === first.name && args[2] === "versions",
		);
		expect(calls).toHaveLength(3);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after registry visibility polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.visibilityDelays = { [INVENTORY[0].name]: 31 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("registry visibility did not converge after 31 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after latest-tag polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.tagVisibilityDelays = { [INVENTORY[0].name]: 31 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("registry visibility did not converge after 31 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	}, 10_000);

	it("stops after attestation-metadata polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.attestationDelays = { [INVENTORY[0].name]: 31 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("attestation metadata did not converge after 31 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("another five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	function publishedState(): FakeState {
		const state = initialState();
		for (const { name, version } of INVENTORY) {
			state.packages[name].versions.push(version);
			state.packages[name].distTags.latest = version;
		}
		return state;
	}

	function expectVerificationPathsRemoved(state: FakeState) {
		expect(state.verificationPaths).toBeDefined();
		for (const path of Object.values(state.verificationPaths!)) expect(existsSync(path)).toBe(false);
	}

	it.each([
		["attestation URL", { missingAttestation: INVENTORY[2].name }, "has no attestation URL"],
		[
			"SLSA provenance predicate",
			{ wrongAttestationPredicate: INVENTORY[2].name },
			"has no SLSA v1 provenance predicate",
		],
	] as const)(
		"requires an exact %s even when native signature audit would succeed",
		(_label, override, message) => {
			writeFileSync(statePath, JSON.stringify(Object.assign(publishedState(), override)));
			const result = runHelper("verify", statePath);
			expect(result.status).not.toBe(0);
			expect(result.stderr).toContain(`${INVENTORY[2].name}@${INVENTORY[2].version} ${message}`);
			expect(result.stderr).toContain("publication state is ambiguous");
			expect(result.stderr).toContain("another five-fresh forward release");
			expect(readState(statePath).calls.some((args) => args[0] === "audit")).toBe(false);
		},
		10_000,
	);

	it("verifies metadata, a normal exact install, native signatures, installed closure, and the CLI", () => {
		writeFileSync(statePath, JSON.stringify(publishedState()));
		const result = runHelper("verify", statePath);
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const state = readState(statePath);
		const install = state.calls.find(([command]) => command === "install")!;
		expect(install).toContain(`@leanandmean/scramjet@${SCRAMJET_VERSION}`);
		expect(install).toContain("--ignore-scripts=false");
		expect(state.calls).toContainEqual(["audit", "signatures", "--registry", "https://registry.npmjs.org/"]);
		expect(state.calls).toContainEqual(["installed-scramjet", "--help"]);
		expect(publishCalls(state)).toHaveLength(0);
		expectVerificationPathsRemoved(state);
	});

	it.each([
		["install", { installFailure: true }, "install failed", false],
		[
			"installed closure",
			{ installedVersionOverrides: { [INVENTORY[0].name]: "0.0.0" } },
			"installed version",
			false,
		],
		["signature audit", { auditFailure: true }, "audit failed", false],
		["CLI", { cliFailure: true }, "installed scramjet --help failed", true],
	] as const)("fails %s verification and removes all owned temporary state", (_label, overrides, message, cliRan) => {
		const state = Object.assign(publishedState(), overrides);
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(message);
		const finalState = readState(statePath);
		expect(finalState.calls.some(([command]) => command === "installed-scramjet")).toBe(cliRan);
		expectVerificationPathsRemoved(finalState);
	});
});

describe("release operation bounds and post-publish polling", () => {
	it("terminates a bounded external read", () => {
		expect(() => run(process.execPath, ["-e", "setTimeout(() => {}, 1000)"], { timeout: 10 })).toThrow();
	});

	it.each([
		["timed-out", Object.assign(new Error("timed out"), { code: "ETIMEDOUT" })],
		["ordinary nonzero", Object.assign(new Error("command failed"), { stderr: "network reset after upload" })],
	])("reports a %s publish failure as ambiguous without retrying", (_label, failure) => {
		const calls: any[][] = [];
		let thrown: Error | undefined;
		try {
			publishPackage(
				INVENTORY[0],
				(...args: any[]) => {
					calls.push(args);
					throw failure;
				},
				25,
			);
		} catch (error) {
			thrown = error as Error;
		}
		expect(thrown?.message).toMatch(/state is ambiguous.*Do not retry publication.*five-fresh forward release/);
		expect((thrown as Error & { cause?: unknown }).cause).toBe(failure);
		expect(calls).toHaveLength(1);
		expect(calls[0][2]).toMatchObject({ timeout: 25 });
	});

	it("uses the production polling interval by default", async () => {
		let attempts = 0;
		const sleep = vi.fn(async () => {});
		await pollRead(
			"published package",
			async () => {
				attempts += 1;
				if (attempts === 1) throw new Error("not ready");
			},
			{ sleep },
		);
		expect(sleep).toHaveBeenCalledOnce();
		expect(sleep).toHaveBeenCalledWith(10_000);
	});

	it("tolerates delayed registry visibility", async () => {
		let attempts = 0;
		const result = await pollRead(
			"published package",
			async () => {
				attempts += 1;
				if (attempts === 1) throw Object.assign(new Error("timed out"), { code: "ETIMEDOUT" });
				if (attempts === 2) throw new DOMException("fetch timed out", "TimeoutError");
				return "verified";
			},
			{ attempts: 3, delayMs: 0, sleep: async () => {}, retryIf: isTransientReadError },
		);
		expect(result).toBe("verified");
		expect(attempts).toBe(3);
	});

	it("fails after the bounded read attempts are exhausted", async () => {
		let attempts = 0;
		await expect(
			pollRead(
				"published package",
				async () => {
					attempts += 1;
					throw new DOMException("still missing", "TimeoutError");
				},
				{ attempts: 3, delayMs: 0, sleep: async () => {}, retryIf: isTransientReadError },
			),
		).rejects.toThrow(/did not converge after 3 attempts: still missing/);
		expect(attempts).toBe(3);
	});
});
