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
	loadPreflightInventory,
	packCandidates,
	pollRead,
	publishPackage,
	reconcileCandidates,
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
	GITHUB_RUN_ID: "36056969151",
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
	registryIntegrity?: Record<string, string>;
	preserveRegistryIntegrity?: string;
	packVariation?: string;
	distOverrides?: Record<string, Record<string, unknown>>;
	distDelays?: Record<string, number>;
	versionOmissions?: Record<string, number>;
	latestDelays?: Record<string, number>;
	latestContradiction?: string;
	packOutput?: { name: string; override: Record<string, unknown> };
	packManifest?: { name: string; field: string; value: unknown };
	packFailure?: string;
	packFileMissing?: string;
	packSymlink?: string;
	targets: Record<string, { name: string; version: string }>;
	calls: string[][];
	failure?: {
		name: string;
		field: string;
		output?: string;
		stderrOutput?: string;
		status?: number;
		remaining?: number;
	};
	failureAfterPublish?: {
		name: string;
		field: string;
		output?: string;
		stderrOutput?: string;
		status?: number;
		remaining?: number;
	};
	publishFailure?: { name: string; mode: "before-landing" | "after-landing-403" | "after-landing-error" };
	publishInputs?: Array<{ path: string; integrity: string }>;
	race?: string;
	raceAt?: number;
	tamperAt?: string;
	packDirectory?: string;
	unexpectedTagChange?: string;
	laterTagChange?: string;
	registryReadMs?: Record<string, number>;
	registryTimeouts?: Array<{ field: string; timeout: number; elapsedMs: number }>;
	versionQueries?: Record<string, number>;
	publicationCounts?: Record<string, number>;
	prePublishLatest?: Record<string, string>;
	visibilityDelays?: Record<string, number>;
	verificationVisibilityDelays?: Record<string, number>;
	tagVisibilityDelays?: Record<string, number>;
	attestationDelays?: Record<string, number>;
	missingAttestation?: string;
	wrongAttestationPredicate?: string;
	installFailure?: boolean;
	installFailures?: Array<{ code: string; summary?: string; stderr?: string; stdout?: string }>;
	installDurationMs?: number;
	installTimeouts?: number[];
	verificationRoots?: string[];
	auditFailure?: boolean | "transient";
	runtimeSmokeFailure?: boolean;
	cliFailure?: boolean;
	installedVersionOverrides?: Record<string, string>;
	verificationPaths?: {
		project: string;
		cache: string;
		home: string;
		xdg: string;
		smoke?: string;
		packageRoot?: string;
	};
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
  if (state.failure?.name === name && state.failure?.field === field && (state.failure.remaining ?? 1) > 0) {
    if (state.failure.remaining !== undefined) state.failure.remaining -= 1;
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
  if (field === "dist") {
    const version = spec.slice(versionSeparator + 1);
    if (!pkg.versions.includes(version)) { save(); process.stdout.write(JSON.stringify({ error: { code: "E404" } })); process.exit(1); }
    if ((state.distDelays?.[name] ?? 0) > 0) {
      state.distDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify({})); process.exit(0);
    }
    if ((state.attestationDelays?.[name] ?? 0) > 0) {
      state.attestationDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify({ integrity: state.registryIntegrity?.[name] })); process.exit(0);
    }
    const dist = state.distOverrides?.[name] ?? {
      integrity: state.registryIntegrity?.[name],
      attestations: state.missingAttestation === name ? {} : {
        url: "https://registry.npmjs.org/fake/" + encodeURIComponent(name) + "/" + version,
        provenance: { predicateType: state.wrongAttestationPredicate === name ? "https://example.test/predicate" : "https://slsa.dev/provenance/v1" },
      },
    };
    save(); process.stdout.write(JSON.stringify(dist)); process.exit(0);
  }
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
    if (state.tamperAt === name && state.versionQueries[name] === 3) {
      const filename = target.name.slice(1).replace("/", "-") + "-" + target.version + ".tgz";
      fs.appendFileSync(require("node:path").join(state.packDirectory, filename), "tampered");
    }
    if (state.race === name && state.versionQueries[name] === (state.raceAt ?? 3)) {
      pkg.versions.push(target.version);
      pkg.distTags.latest = target.version;
    }
    if ((state.publicationCounts?.[name] ?? 0) > 0 && (state.visibilityDelays?.[name] ?? 0) > 0) {
      state.visibilityDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify(pkg.versions.filter((version) => version !== target.version))); process.exit(0);
    }
    if ((state.versionOmissions?.[name] ?? 0) > 0) {
      state.versionOmissions[name] -= 1;
      save(); process.stdout.write(JSON.stringify(pkg.versions.filter((version) => version !== target.version))); process.exit(0);
    }
    save(); process.stdout.write(JSON.stringify(pkg.versions)); process.exit(0);
  }
  if (field === "dist-tags") {
    if (state.latestContradiction === name && state.packDirectory) {
      delete state.latestContradiction;
      save(); process.stdout.write(JSON.stringify({ ...pkg.distTags, latest: state.targets[Object.keys(state.targets).find(workspace => state.targets[workspace].name === name)].version })); process.exit(0);
    }
    if ((state.latestDelays?.[name] ?? 0) > 0) {
      state.latestDelays[name] -= 1;
      const previous = { ...pkg.distTags };
      if (state.latestDelays[name] === 0) pkg.distTags.latest = state.targets[Object.keys(state.targets).find(workspace => state.targets[workspace].name === name)].version;
      save(); process.stdout.write(JSON.stringify(previous)); process.exit(0);
    }
    if ((state.publicationCounts?.[name] ?? 0) > 0 && (state.tagVisibilityDelays?.[name] ?? 0) > 0) {
      state.tagVisibilityDelays[name] -= 1;
      save(); process.stdout.write(JSON.stringify({ ...pkg.distTags, latest: state.prePublishLatest[name] })); process.exit(0);
    }
    save(); process.stdout.write(JSON.stringify(pkg.distTags)); process.exit(0);
  }
  stop("unexpected view");
}
if (args[0] === "pack") {
  const workspace = args[args.indexOf("-w") + 1];
  const target = state.targets[workspace];
  if (!target) stop("unknown workspace");
  if (state.packFailure === target.name) stop("pack failed for " + target.name);
  const destination = args[args.indexOf("--pack-destination") + 1];
  const filename = target.name.slice(1).replace("/", "-") + "-" + target.version + ".tgz";
  const source = fs.mkdtempSync(require("node:os").tmpdir() + "/scramjet-pack-fixture-");
  try {
    fs.mkdirSync(require("node:path").join(source, "package"));
    const manifest = JSON.parse(fs.readFileSync(require("node:path").join(process.cwd(), workspace, "package.json"), "utf8"));
    if (state.packManifest?.name === target.name) manifest[state.packManifest.field] = state.packManifest.value;
    fs.writeFileSync(require("node:path").join(source, "package", "package.json"), JSON.stringify(manifest));
    if (state.packVariation === target.name) fs.writeFileSync(require("node:path").join(source, "package", "variation"), "changed");
    const archive = require("node:path").join(destination, filename);
    require("node:child_process").execFileSync("tar", ["-czf", archive, state.packVariation === target.name ? "package" : "package/package.json"], { cwd: source });
    if (state.packSymlink === target.name) {
      fs.renameSync(archive, archive + ".original");
      fs.symlinkSync(archive + ".original", archive);
    }
    const integrity = "sha512-" + createHash("sha512").update(fs.readFileSync(archive)).digest("base64");
    state.registryIntegrity ??= {};
    if (state.preserveRegistryIntegrity !== target.name) state.registryIntegrity[target.name] = integrity;
    state.packDirectory = destination;
    if (state.packFileMissing === target.name) fs.unlinkSync(archive);
    const override = state.packOutput?.name === target.name ? state.packOutput.override : {};
    const result = { name: target.name, version: target.version, filename, integrity, ...override };
    save(); process.stdout.write(override.invalidJson ? "not json" : JSON.stringify(override.empty ? [] : override.duplicate ? [result, result] : [result]));
  } finally { fs.rmSync(source, { recursive: true, force: true }); }
  process.exit(0);
}
if (args[0] === "publish") {
  const archive = args[1];
  const target = Object.values(state.targets).find(t => archive.endsWith(t.name.slice(1).replace("/", "-") + "-" + t.version + ".tgz"));
  if (!target || !require("node:path").isAbsolute(archive)) stop("invalid candidate archive");
  const integrity = "sha512-" + createHash("sha512").update(fs.readFileSync(archive)).digest("base64");
  state.publishInputs ??= [];
  state.publishInputs.push({ path: archive, integrity });
  if (integrity !== state.registryIntegrity?.[target.name]) stop("candidate bytes differ");
  const failure = state.publishFailure?.name === target.name ? state.publishFailure.mode : undefined;
  if (failure === "before-landing") stop("publish failed before landing");
  const pkg = state.packages[target.name];
  state.publicationCounts ??= {};
  state.prePublishLatest ??= {};
  state.publicationCounts[target.name] = (state.publicationCounts[target.name] ?? 0) + 1;
  state.prePublishLatest[target.name] = pkg.distTags.latest;
  pkg.versions.push(target.version);
  pkg.distTags.latest = target.version;
  if (state.unexpectedTagChange === target.name) pkg.distTags.scramjet = target.version;
  if (state.laterTagChange) state.packages[state.laterTagChange].distTags.scramjet = "changed";
  save();
  if (failure) { console.error(failure === "after-landing-403" ? "E403: publish failed" : "publish command errored"); process.exit(1); }
  process.stdout.write("published"); process.exit(0);
}
if (args[0] === "install") {
  state.verificationPaths = {
    project: process.cwd(),
    cache: process.env.npm_config_cache,
    home: process.env.HOME,
    xdg: process.env.XDG_DATA_HOME,
  };
  state.verificationRoots ??= [];
  state.verificationRoots.push(require("node:path").dirname(process.cwd()));
  const failure = state.installFailures?.shift();
  if (failure) {
    save();
    if (failure.stderr) console.error(failure.stderr);
    process.stdout.write(failure.stdout ?? JSON.stringify({ error: { code: failure.code, summary: failure.summary } }));
    process.exit(1);
  }
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
  if (state.auditFailure === "transient") { save(); process.stdout.write(JSON.stringify({ error: { code: "E408" } })); process.exit(1); }
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

function runHelper(
	mode: string,
	statePath: string,
	args = mode === "preflight" ? [SHA] : [],
	environment: Record<string, string> = {},
) {
	const script = [
		"publish",
		"publish-and-verify",
		"registry-preflight",
		"verify",
		"verify-delayed",
		"verify-cli",
	].includes(mode)
		? join(dirname(statePath), "runner.mjs")
		: HELPER;
	return spawnSync(process.execPath, [script, mode === "publish-cli" ? "publish" : mode, ...args], {
		cwd: REPO_ROOT,
		encoding: "utf8",
		env: {
			...process.env,
			...RELEASE_ENV,
			...environment,
			PATH: `${dirname(statePath)}:${process.env.PATH}`,
			FAKE_NPM_STATE: statePath,
			FAKE_NPM_SCRIPT: join(dirname(statePath), "npm"),
		},
	});
}

function readState(path: string): FakeState {
	return JSON.parse(readFileSync(path, "utf8"));
}

function publishCalls(state: FakeState): string[][] {
	return state.calls.filter(([command]) => command === "publish");
}

function expectFirstPackageObservationFailureSummary(stderr: string) {
	expect(stderr).toContain(`${INVENTORY[0].name}@${INVENTORY[0].version}: command accepted; observation incomplete`);
	for (const { name, version } of INVENTORY.slice(1)) expect(stderr).toContain(`${name}@${version}: unattempted`);
	expect(stderr).toContain("failed phase: post-publish observation");
	expect(stderr).toContain("final verification: not completed");
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
		expect(validateIdentity(INVENTORY, RELEASE_ENV, () => SHA)).toEqual({
			ref: RELEASE_ENV.GITHUB_REF,
			sha: SHA,
			runId: RELEASE_ENV.GITHUB_RUN_ID,
			attempt: "1",
		});
	});

	it("accepts a canonical later attempt with run correlation", () => {
		expect(validateIdentity(INVENTORY, { ...RELEASE_ENV, GITHUB_RUN_ATTEMPT: "2" }, () => SHA)).toEqual({
			ref: RELEASE_ENV.GITHUB_REF,
			sha: SHA,
			runId: RELEASE_ENV.GITHUB_RUN_ID,
			attempt: "2",
		});
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
		["missing run ID", { GITHUB_RUN_ID: undefined }],
		["noncanonical run ID", { GITHUB_RUN_ID: "01" }],
		["zero attempt", { GITHUB_RUN_ATTEMPT: "0" }],
		["noncanonical attempt", { GITHUB_RUN_ATTEMPT: "02" }],
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
			`import { readFileSync, realpathSync, writeFileSync } from "node:fs";
import childProcess from "node:child_process";
import { createRequire, syncBuiltinESMExports } from "node:module";
import { runInNewContext } from "node:vm";
let elapsedMs = 0;
if (process.env.IN_PROCESS_NPM === "1") {
  const realExec = childProcess.execFileSync;
  childProcess.execFileSync = (command, args, options) => {
    if (command === "git") return realExec(command, args, options);
    const cli = command.endsWith("/node_modules/.bin/scramjet");
    if (command === "tar") return realExec(command, args, options);
    if (command === process.execPath && args[0].endsWith("/installed-runtime-smoke.mjs")) {
      const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
      state.calls.push(["installed-runtime-smoke", ...args.slice(1)]);
      writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
      return "";
    }
    if (command !== "npm" && !cli) throw new Error("Unexpected fixture command: " + command);
    const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
    if (command === "npm" && args[0] === "install") {
      state.installTimeouts ??= [];
      state.installTimeouts.push(options.timeout);
      elapsedMs += state.installDurationMs ?? 0;
      writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
    }
    if (command === "npm" && args[0] === "view" && Object.values(state.publicationCounts ?? {}).some(count => count > 0)) {
      state.registryTimeouts ??= [];
      state.registryTimeouts.push({ field: args[2], timeout: options.timeout, elapsedMs });
      elapsedMs += state.registryReadMs?.[args[2]] ?? 0;
      writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
    }
    let stdout = "";
    let stderr = "";
    let status = 0;
    const exited = {};
    const source = readFileSync(cli ? command : process.env.FAKE_NPM_SCRIPT, "utf8");
    try {
      runInNewContext(source, {
        require: createRequire(import.meta.url),
        console: { error: message => { stderr += message; } },
        process: {
          argv: [process.execPath, command, ...args],
          env: { ...process.env, ...options.env },
          cwd: () => options.cwd,
          stdout: { write: text => { stdout += text; } },
          exit: code => { status = code; throw exited; },
        },
      });
    } catch (error) {
      if (error !== exited) throw error;
    }
    if (status !== 0) throw Object.assign(new Error("fixture command failed"), { status, stdout, stderr });
    return stdout;
  };
  syncBuiltinESMExports();
}
const productionVerify = process.argv[2] === "verify-cli";
if (productionVerify) { process.argv[1] = ${JSON.stringify(HELPER)}; process.argv[2] = "verify"; }
const { loadInventory, preflight, publish, validateIdentity, verify } = await import(${JSON.stringify(new URL("../../../.github/scripts/release.mjs", import.meta.url))});
if (!productionVerify) try {
  const inventory = loadInventory();
  const identity = validateIdentity(inventory);
  if (["publish", "publish-and-verify"].includes(process.argv[2]) && identity.attempt === "1") preflight(inventory);
  const pollDependencies = {
    budgetMs: 600_000,
    delayMs: Number(process.env.POLL_DELAY_MS ?? 10_000),
    now: () => elapsedMs,
    sleep: async (duration) => { elapsedMs += duration; },
  };
  const runInstalledRuntimeSmoke = (packageRoot, workDir) => {
    const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
    state.calls.push(["installed-runtime-smoke", packageRoot, workDir]);
    state.verificationPaths.smoke = workDir;
    state.verificationPaths.packageRoot = realpathSync(packageRoot);
    writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
    if (state.runtimeSmokeFailure) throw Object.assign(new Error("runtime smoke failed"), { stderr: "runtime smoke failed" });
  };
  if (process.argv[2] === "publish") await publish(inventory, { pollDependencies });
  else if (process.argv[2] === "publish-and-verify") {
    await publish(inventory, { pollDependencies });
    await verify(inventory, { pollDependencies, runInstalledRuntimeSmoke });
  } else if (process.argv[2] === "verify-delayed") {
    await verify(inventory, {
      pollDependencies,
      runInstalledRuntimeSmoke,
      readVerificationMetadata: (pkg) => {
        const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
        state.calls.push(["verification-metadata", pkg.name]);
        const remaining = state.verificationVisibilityDelays?.[pkg.name] ?? 0;
        if (remaining > 0) state.verificationVisibilityDelays[pkg.name] = remaining - 1;
        writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
        if (remaining > 0) throw Object.assign(new Error(pkg.name + " is not visible"), { code: "REGISTRY_PROPAGATION" });
      },
    });
  } else if (process.argv[2] === "verify") await verify(inventory, { pollDependencies, runInstalledRuntimeSmoke });
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

	async function withCandidates(
		check: (
			candidates: Array<(typeof INVENTORY)[number] & { tarballPath: string; integrity: string }>,
			state: FakeState,
		) => Promise<void> | void,
	) {
		const oldPath = process.env.PATH;
		const oldState = process.env.FAKE_NPM_STATE;
		process.env.PATH = `${workDir}:${oldPath}`;
		process.env.FAKE_NPM_STATE = statePath;
		try {
			const directory = join(workDir, "candidates");
			mkdirSync(directory, { recursive: true });
			const candidates = packCandidates(INVENTORY, directory);
			const state = readState(statePath);
			state.registryIntegrity = Object.fromEntries(candidates.map(({ name, integrity }) => [name, integrity]));
			writeFileSync(statePath, JSON.stringify(state));
			await check(candidates, state);
		} finally {
			if (oldPath === undefined) delete process.env.PATH;
			else process.env.PATH = oldPath;
			if (oldState === undefined) delete process.env.FAKE_NPM_STATE;
			else process.env.FAKE_NPM_STATE = oldState;
		}
	}

	function clock(budgetMs = 5_000) {
		let elapsedMs = 0;
		return {
			budgetMs,
			delayMs: 1_000,
			now: () => elapsedMs,
			sleep: async (duration: number) => {
				elapsedMs += duration;
			},
		};
	}

	function markPresent(state: FakeState, ...indexes: number[]) {
		for (const index of indexes) {
			const { name, version } = INVENTORY[index];
			state.packages[name].versions.push(version);
			state.packages[name].distTags.latest = version;
		}
		writeFileSync(statePath, JSON.stringify(state));
	}

	it("packs all five real archives with matching bytes and canonical packed manifests", async () => {
		await withCandidates((candidates, state) => {
			expect(candidates.map(({ workspace }) => workspace)).toEqual(INVENTORY.map(({ workspace }) => workspace));
			for (const candidate of candidates) {
				expect(candidate.tarballPath).toBe(
					join(workDir, "candidates", `${candidate.name.slice(1).replace("/", "-")}-${candidate.version}.tgz`),
				);
				expect(candidate.integrity).toMatch(/^sha512-/);
				expect(existsSync(candidate.tarballPath)).toBe(true);
			}
			expect(state.calls.filter(([command]) => command === "pack")).toHaveLength(5);
			expect(state.calls.some(([command]) => command === "publish")).toBe(false);
		});
	});

	it.each([
		["malformed JSON", { invalidJson: true }],
		["no pack results", { empty: true }],
		["multiple pack results", { duplicate: true }],
		["wrong name", { name: "@leanandmean/other" }],
		["wrong version", { version: "0.0.0" }],
		["unsafe filename", { filename: "../../outside.tgz" }],
		["invalid integrity", { integrity: "sha512-AAAA" }],
		["different reported digest", { integrity: `sha512-${Buffer.alloc(64).toString("base64")}` }],
	])("rejects %s from npm pack", (_label, override) => {
		const state = initialState();
		state.packOutput = { name: INVENTORY[1].name, override };
		writeFileSync(statePath, JSON.stringify(state));
		const oldPath = process.env.PATH;
		process.env.PATH = `${workDir}:${oldPath}`;
		try {
			expect(() => packCandidates(INVENTORY, workDir)).toThrow();
		} finally {
			process.env.PATH = oldPath;
		}
		expect(readState(statePath).calls.some(([command]) => command === "publish")).toBe(false);
	});

	it("rejects failed pack, symlink archives and incorrect archived manifests", () => {
		for (const change of [
			{ packFailure: INVENTORY[0].name },
			{ packFileMissing: INVENTORY[0].name },
			{ packSymlink: INVENTORY[0].name },
			{ packManifest: { name: INVENTORY[0].name, field: "name", value: "@leanandmean/other" } },
			{ packManifest: { name: INVENTORY[0].name, field: "version", value: "0.0.0" } },
			{ packManifest: { name: INVENTORY[0].name, field: "repository", value: "untrusted" } },
			{ packManifest: { name: INVENTORY[0].name, field: "publishConfig", value: { access: "restricted" } } },
			{ packManifest: { name: INVENTORY[2].name, field: "dependencies", value: {} } },
		]) {
			writeFileSync(statePath, JSON.stringify({ ...initialState(), ...change }));
			const oldPath = process.env.PATH;
			process.env.PATH = `${workDir}:${oldPath}`;
			try {
				expect(() => packCandidates(INVENTORY, workDir)).toThrow();
			} finally {
				process.env.PATH = oldPath;
			}
		}
	});

	it("reconciles all missing, all matching and non-prefix retained candidates without publishing", async () => {
		await withCandidates(async (candidates, state) => {
			const missing = await reconcileCandidates(candidates, clock());
			expect(missing.map(({ status }) => status)).toEqual(INVENTORY.map(() => "missing"));
			markPresent(state, 0, 1, 2, 3, 4);
			const retained = await reconcileCandidates(candidates, clock());
			expect(retained.map(({ status }) => status)).toEqual(INVENTORY.map(() => "retained"));
			expect(retained[0].distTags).toEqual({ latest: INVENTORY[0].version, scramjet: "preserved" });
			state.packages[INVENTORY[1].name].versions.pop();
			state.packages[INVENTORY[1].name].distTags.latest = previousVersion(INVENTORY[1].version);
			state.packages[INVENTORY[3].name].versions.pop();
			state.packages[INVENTORY[3].name].distTags.latest = previousVersion(INVENTORY[3].version);
			writeFileSync(statePath, JSON.stringify(state));
			expect((await reconcileCandidates(candidates, clock())).map(({ status }) => status)).toEqual([
				"retained",
				"missing",
				"retained",
				"missing",
				"retained",
			]);
			expect(readState(statePath).calls.some(([command]) => command === "publish")).toBe(false);
		});
	});

	it("stops a valid content mismatch or a superseding latest without mutation", async () => {
		await withCandidates(async (candidates, state) => {
			markPresent(state, 0);
			state.registryIntegrity![INVENTORY[0].name] = `sha512-${Buffer.alloc(64).toString("base64")}`;
			writeFileSync(statePath, JSON.stringify(state));
			await expect(reconcileCandidates(candidates, clock())).rejects.toThrow(/integrity differs/);
			state.registryIntegrity![INVENTORY[0].name] = candidates[0].integrity;
			state.packages[INVENTORY[0].name].distTags.latest = "999.0.0";
			writeFileSync(statePath, JSON.stringify(state));
			await expect(reconcileCandidates(candidates, clock())).rejects.toThrow(/superseded/);
			expect(readState(statePath).calls.some(([command]) => command === "publish")).toBe(false);
		});
	});

	it("bounds delayed digest, attestation and contradictory omission under one clock", async () => {
		await withCandidates(async (candidates, state) => {
			markPresent(state, 0);
			state.distDelays = { [INVENTORY[0].name]: 1 };
			state.attestationDelays = { [INVENTORY[0].name]: 1 };
			state.versionOmissions = { [INVENTORY[0].name]: 1 };
			writeFileSync(statePath, JSON.stringify(state));
			const result = await reconcileCandidates(candidates, clock(7_000));
			expect(result[0].status).toBe("retained");
			const finalState = readState(statePath);
			expect(finalState.versionOmissions?.[INVENTORY[0].name]).toBe(0);
			expect(finalState.distDelays?.[INVENTORY[0].name]).toBe(0);
			expect(finalState.attestationDelays?.[INVENTORY[0].name]).toBe(0);
		});
	});

	it("waits for latest to catch up with a visible matching target", async () => {
		await withCandidates(async (candidates, state) => {
			markPresent(state, 0);
			state.packages[INVENTORY[0].name].distTags.latest = previousVersion(INVENTORY[0].version);
			state.latestDelays = { [INVENTORY[0].name]: 1 };
			writeFileSync(statePath, JSON.stringify(state));
			const result = await reconcileCandidates([candidates[0]], clock());
			expect(result[0].status).toBe("retained");
			expect(result[0].distTags.latest).toBe(INVENTORY[0].version);
		});
	});

	it("does not publish after latest briefly names a target omitted by versions", () => {
		const state = initialState();
		state.latestContradiction = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${INVENTORY[0].name}@${INVENTORY[0].version}`);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("never turns observed presence into absence after a later versions omission", async () => {
		await withCandidates(async (candidates, state) => {
			markPresent(state, 0);
			state.distDelays = { [INVENTORY[0].name]: 1 };
			state.versionOmissions = { [INVENTORY[0].name]: 0 };
			writeFileSync(statePath, JSON.stringify(state));
			const seen = new Set<string>();
			await reconcileCandidates([candidates[0]], { ...clock(), observedPresence: seen });
			state.versionOmissions[INVENTORY[0].name] = 10;
			state.packages[INVENTORY[0].name].distTags.latest = previousVersion(INVENTORY[0].version);
			writeFileSync(statePath, JSON.stringify(state));
			await expect(reconcileCandidates([candidates[0]], { ...clock(), observedPresence: seen })).rejects.toThrow(
				/did not converge/,
			);
			expect(seen.has(INVENTORY[0].name)).toBe(true);
		});
	});

	it("does not infer absence from a failed or malformed versions lookup", async () => {
		await withCandidates(async (candidates, state) => {
			for (const failure of [
				{
					name: INVENTORY[0].name,
					field: "versions",
					status: 1,
					output: JSON.stringify({ error: { code: "E401" } }),
				},
				{ name: INVENTORY[0].name, field: "versions", output: "not json" },
			]) {
				state.failure = failure;
				writeFileSync(statePath, JSON.stringify(state));
				await expect(reconcileCandidates(candidates, clock())).rejects.toThrow();
			}
			expect(readState(statePath).calls.some(([command]) => command === "publish")).toBe(false);
		});
	});

	it("retains a structured transport cause on observation exhaustion", async () => {
		await withCandidates(async (candidates, state) => {
			state.failure = {
				name: INVENTORY[0].name,
				field: "versions",
				status: 1,
				output: JSON.stringify({ error: { code: "E429" } }),
			};
			writeFileSync(statePath, JSON.stringify(state));
			try {
				await reconcileCandidates(candidates, clock());
				throw new Error("expected exhaustion");
			} catch (error) {
				expect(error).toHaveProperty("cause");
				expect((error as Error).message).toMatch(/did not converge within 5000ms/);
				expect((error as Error & { cause: Error }).cause).toHaveProperty("stdout", expect.stringContaining("E429"));
			}
		});
	});

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
			copyReleaseMetadata(root);
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
			expect(() => loadPreflightInventory(head, git)).toThrow(
				"release manifests and package-lock.json must match checked-out HEAD",
			);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("resumes partial publication on attempt 2 without republishing matching targets", () => {
		const first = runHelper("publish", statePath);
		expect(first.status).toBe(0);
		const state = readState(statePath);
		for (const { name, version } of INVENTORY.slice(2)) {
			state.packages[name].versions = state.packages[name].versions.filter((value) => value !== version);
			state.packages[name].distTags.latest = previousVersion(version);
		}
		state.calls = [];
		state.publicationCounts = {};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { GITHUB_RUN_ATTEMPT: "2" });
		expect(result.status).toBe(0);
		expect(result.stdout).toContain("matching content retained");
		expect(publishCalls(readState(statePath))).toHaveLength(3);
	});

	it("resumes a failed first attempt from registry state, without a second command in that attempt", () => {
		const state = initialState();
		state.publishFailure = { name: INVENTORY[1].name, mode: "before-landing" };
		writeFileSync(statePath, JSON.stringify(state));
		const failed = runHelper("publish", statePath, [], { IN_PROCESS_NPM: "1", POLL_DELAY_MS: "300000" });
		expect(failed.status).not.toBe(0);
		expect(failed.stderr).toContain("publish failed before landing");
		expect(failed.stderr).toContain("acceptance ambiguous");
		const afterFailure = readState(statePath);
		expect(publishCalls(afterFailure)).toHaveLength(2);
		expect(afterFailure.packages[INVENTORY[0].name].versions).toContain(INVENTORY[0].version);
		delete afterFailure.publishFailure;
		afterFailure.calls = [];
		writeFileSync(statePath, JSON.stringify(afterFailure));
		const resumed = runHelper("publish", statePath, [], { GITHUB_RUN_ATTEMPT: "2" });
		expect(resumed.status).toBe(0);
		expect(resumed.stdout).toContain(`${INVENTORY[0].name}@${INVENTORY[0].version}: matching content retained`);
		expect(publishCalls(readState(statePath))).toHaveLength(4);
	});

	it.each(["after-landing-403", "after-landing-error"] as const)(
		"observes a %s publish error without retrying the command",
		(mode) => {
			const state = initialState();
			state.publishFailure = { name: INVENTORY[0].name, mode };
			writeFileSync(statePath, JSON.stringify(state));
			const result = runHelper("publish", statePath);
			expect(result.status).toBe(0);
			expect(result.stderr).toContain(mode === "after-landing-403" ? "E403" : "publish command errored");
			expect(result.stdout).toContain(
				`${INVENTORY[0].name}@${INVENTORY[0].version}: command errored; matching content observed`,
			);
			expect(publishCalls(readState(statePath))).toHaveLength(5);
		},
	);

	it("stops on a changed checked archive before invoking npm publish", () => {
		const state = initialState();
		state.tamperAt = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("candidate archive changed before publication");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("runs the production publish CLI against checked tarball files", () => {
		const result = runHelper("publish-cli", statePath);
		expect(result.status).toBe(0);
		const state = readState(statePath);
		expect(state.calls.filter(([command]) => command === "pack")).toHaveLength(5);
		expect(publishCalls(state)).toHaveLength(5);
		for (const { path, integrity } of state.publishInputs ?? []) {
			expect(path).toMatch(/^\/.*\.tgz$/);
			expect(Object.values(state.registryIntegrity ?? {})).toContain(integrity);
			expect(existsSync(path)).toBe(false);
		}
		expect(result.stdout).toContain("release run 36056969151 attempt 1");
		expect(result.stdout).toContain("final verification: pending");
	});

	it("production CLI accepts attempt 2 only after rechecking committed inventory and retained content", () => {
		const first = runHelper("publish-cli", statePath);
		expect(first.status).toBe(0);
		const state = readState(statePath);
		state.calls = [];
		const last = INVENTORY.at(-1)!;
		state.packages[last.name].versions = state.packages[last.name].versions.filter(
			(version) => version !== last.version,
		);
		state.packages[last.name].distTags.latest = previousVersion(last.version);
		writeFileSync(statePath, JSON.stringify(state));
		const resumed = runHelper("publish-cli", statePath, [], { GITHUB_RUN_ATTEMPT: "2" });
		expect(resumed.status).toBe(0);
		expect(resumed.stdout).toContain("release run 36056969151 attempt 2");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	}, 20_000);

	it("production CLI rejects changed candidate bytes against a retained registry digest", () => {
		const first = runHelper("publish-cli", statePath);
		expect(first.status).toBe(0);
		const state = readState(statePath);
		const retained = INVENTORY[0].name;
		const originalDigest = state.registryIntegrity![retained];
		state.preserveRegistryIntegrity = retained;
		state.packVariation = retained;
		state.calls = [];
		writeFileSync(statePath, JSON.stringify(state));
		const resumed = runHelper("publish-cli", statePath, [], { GITHUB_RUN_ATTEMPT: "2" });
		expect(resumed.status).not.toBe(0);
		expect(resumed.stderr).toMatch(/integrity|digest|content|mismatch/i);
		const after = readState(statePath);
		expect(after.calls.filter(([command]) => command === "pack")).toHaveLength(5);
		expect(after.registryIntegrity![retained]).toBe(originalDigest);
		expect(publishCalls(after)).toHaveLength(0);
	});

	it("production CLI rejects a present target on attempt 1 before packing or publishing", () => {
		const state = initialState();
		const first = INVENTORY[0];
		state.packages[first.name].versions.push(first.version);
		state.packages[first.name].distTags.latest = first.version;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish-cli", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toMatch(/already|present|fresh|preflight|published/i);
		const after = readState(statePath);
		expect(after.calls.filter(([command]) => command === "pack")).toHaveLength(0);
		expect(publishCalls(after)).toHaveLength(0);
	});

	it("production CLI captures a real-child error after landing and still observes matching content", () => {
		const state = initialState();
		state.publishFailure = { name: INVENTORY[0].name, mode: "after-landing-403" };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish-cli", statePath);
		expect(result.status).toBe(0);
		expect(result.stderr).toContain("E403: publish failed");
		expect(result.stdout).toContain("command errored; matching content observed");
		expect(publishCalls(readState(statePath))).toHaveLength(5);
	});

	it("rejects invalid production CLI identity before packing or registry access", () => {
		const result = runHelper("publish-cli", statePath, [], { GITHUB_RUN_ATTEMPT: "02" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("canonical positive decimal attempt");
		expect(readState(statePath).calls).toHaveLength(0);
	});

	it("publishes all missing packages in dependency order with explicit latest and provenance", () => {
		const result = runHelper("publish", statePath);
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const state = readState(statePath);
		const calls = publishCalls(state);
		expect(calls.map((args) => args[1].split("/").at(-1))).toEqual(
			INVENTORY.map(({ name, version }) => `${name.slice(1).replace("/", "-")}-${version}.tgz`),
		);
		for (const args of calls) {
			expect(args).toEqual([
				"publish",
				expect.stringMatching(/^\/.*\.tgz$/),
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
			expect(result.stdout).toContain(`${name}@${version}: command accepted; matching content observed`);
		}
		expect(result.stdout).toContain("publication summary:");
		expect(result.stdout).toContain("final verification: pending");
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

	it("skips a matching target appearing at the immediate reread", () => {
		const state = initialState();
		state.race = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${INVENTORY[0].name}@${INVENTORY[0].version}: matching content retained`);
		expect(publishCalls(readState(statePath))).toHaveLength(4);
	});

	it("stops on a different late candidate after publication began", () => {
		const state = initialState();
		state.race = INVENTORY[1].name;
		state.distOverrides = { [INVENTORY[1].name]: { integrity: `sha512-${Buffer.alloc(64).toString("base64")}` } };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${INVENTORY[1].name}@${INVENTORY[1].version}`);
		expect(result.stderr).toContain("same-run rerun");
		expect(result.stderr).toContain("five-fresh forward release");
		expect(publishCalls(readState(statePath)).map((args) => args[1].split("/").at(-1))).toEqual([
			`${INVENTORY[0].name.slice(1).replace("/", "-")}-${INVENTORY[0].version}.tgz`,
		]);
	});

	it("treats every publish failure as ambiguous without retrying", () => {
		const state = initialState();
		state.publishFailure = { name: INVENTORY[1].name, mode: "before-landing" };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { IN_PROCESS_NPM: "1", POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("acceptance ambiguous");
		expect(result.stderr).toContain("same-run rerun");
		expect(result.stderr).toContain("publish failed");
		expect(result.stderr).toContain(
			`${INVENTORY[0].name}@${INVENTORY[0].version}: command accepted; matching content observed`,
		);
		expect(result.stderr).toContain(`${INVENTORY[1].name}@${INVENTORY[1].version}: acceptance ambiguous`);
		for (const { name, version } of INVENTORY.slice(2))
			expect(result.stderr).toContain(`${name}@${version}: unattempted`);
		expect(result.stderr).toContain("failed phase: post-publish observation");
		expect(result.stderr).toContain("final verification: not completed");
		const calls = publishCalls(readState(statePath));
		expect(calls.map((args) => args[1].split("/").at(-1))).toEqual(
			INVENTORY.slice(0, 2).map(({ name, version }) => `${name.slice(1).replace("/", "-")}-${version}.tgz`),
		);
	});

	it("fails if publication changes a non-latest dist-tag", () => {
		const state = initialState();
		state.unexpectedTagChange = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("non-latest dist-tags changed");
		expect(result.stderr).toContain("command accepted; observation incomplete");
		expect(result.stderr).toContain("five-fresh forward release");
		expectFirstPackageObservationFailureSummary(result.stderr);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("publishes all five packages and verifies after more than 31 stale publication reads", () => {
		const state = initialState();
		state.visibilityDelays = { [INVENTORY[1].name]: 32 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish-and-verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const finalState = readState(statePath);
		expect(publishCalls(finalState).map((args) => args[1].split("/").at(-1))).toEqual(
			INVENTORY.map(({ name, version }) => `${name.slice(1).replace("/", "-")}-${version}.tgz`),
		);
		expect(Object.values(finalState.publicationCounts ?? {})).toEqual(INVENTORY.map(() => 1));
		expect(finalState.versionQueries?.[INVENTORY[1].name]).toBe(37);
		expect(result.stdout).toContain("final verification: completed");
		expectVerificationPathsRemoved(finalState);
		expect(finalState.calls.some(([command]) => command === "install")).toBe(true);
		expect(finalState.calls).toContainEqual(["audit", "signatures", "--registry", "https://registry.npmjs.org/"]);
		expect(finalState.calls.some(([command]) => command === "installed-runtime-smoke")).toBe(true);
		expect(finalState.calls).toContainEqual(["installed-scramjet", "--help"]);
	});

	it("shares one deadline across version and attestation visibility", () => {
		const state = initialState();
		const first = INVENTORY[0].name;
		state.visibilityDelays = { [first]: 3 };
		state.attestationDelays = { [first]: 2 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish-and-verify", statePath, [], {
			IN_PROCESS_NPM: "1",
			POLL_DELAY_MS: "120000",
		});
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("within 600000ms after 600000ms and 5 observations");
		expectFirstPackageObservationFailureSummary(result.stderr);
		const finalState = readState(statePath);
		expect(finalState.visibilityDelays?.[first]).toBe(0);
		expect(finalState.attestationDelays?.[first]).toBe(0);
		expect(publishCalls(finalState)).toHaveLength(1);
		expect(finalState.calls.some(([command]) => command === "install")).toBe(false);
	});

	it("forwards the shrinking shared remainder to actual registry subprocess options", () => {
		const state = initialState();
		state.visibilityDelays = { [INVENTORY[0].name]: 1 };
		state.registryReadMs = { versions: 10000, "dist-tags": 10000, dist: 1000 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { IN_PROCESS_NPM: "1", POLL_DELAY_MS: "550000" });
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const finalState = readState(statePath);
		expect(finalState.registryTimeouts?.slice(0, 4)).toEqual([
			{ field: "versions", timeout: 60000, elapsedMs: 0 },
			{ field: "dist-tags", timeout: 60000, elapsedMs: 10000 },
			{ field: "versions", timeout: 30000, elapsedMs: 570000 },
			{ field: "dist-tags", timeout: 20000, elapsedMs: 580000 },
		]);
		expect(publishCalls(finalState)).toHaveLength(5);
	});

	it("reports forward-only recovery for later pre-publish tag validation failures", () => {
		const state = initialState();
		state.laterTagChange = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(`${INVENTORY[1].name} dist-tags changed after reconciliation`);
		expect(result.stderr).toContain("same-run rerun");
		expect(result.stderr).toContain("five-fresh forward release");
		expect(result.stderr).toContain(
			`${INVENTORY[0].name}@${INVENTORY[0].version}: command accepted; matching content observed`,
		);
		for (const { name, version } of INVENTORY.slice(1))
			expect(result.stderr).toContain(`${name}@${version}: unattempted`);
		expect(result.stderr).toContain("failed phase: immediate reread");
		expect(result.stderr).toContain("final verification: not completed");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("polls multi-minute latest-tag visibility through publication", () => {
		const state = initialState();
		const first = INVENTORY[0].name;
		state.tagVisibilityDelays = { [first]: 3 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { POLL_DELAY_MS: "60000" });
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} candidate reconciliation not ready`);
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
		expect(result.stderr).toContain("observation failed");
		expectFirstPackageObservationFailureSummary(result.stderr);
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === first.name && args[2] === field,
		);
		expect(calls).toHaveLength(4);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("retries an empty attestation URL within the shared package budget", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = {
			name: first.name,
			field: "dist",
			output: JSON.stringify({}),
			remaining: 1,
		};
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first.name}@${first.version} candidate reconciliation not ready`);
		expect(publishCalls(readState(statePath))).toHaveLength(INVENTORY.length);
	});

	it("fails malformed attestation metadata without retrying", () => {
		const first = INVENTORY[0];
		const state = initialState();
		state.failureAfterPublish = { name: first.name, field: "dist", output: "[]" };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("dist must be a JSON object");
		expectFirstPackageObservationFailureSummary(result.stderr);
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === `${first.name}@${first.version}` && args[2] === "dist",
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
		expect(result.stdout).toContain(`${first.name}@${first.version} candidate reconciliation not ready`);
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
		expect(result.stdout).toContain(`${first.name}@${first.version} candidate reconciliation not ready`);
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
		expect(calls).toHaveLength(4);
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
		expect(result.stderr).toContain("observation failed");
		expectFirstPackageObservationFailureSummary(result.stderr);
		const calls = readState(statePath).calls.filter(
			(args) => args[0] === "view" && args[1] === first.name && args[2] === "versions",
		);
		expect(calls).toHaveLength(4);
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after registry visibility polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.visibilityDelays = { [INVENTORY[0].name]: 2 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("candidate reconciliation did not converge within 600000ms");
		expect(result.stderr).toContain("observation failed");
		expect(result.stderr).toContain("five-fresh forward release");
		expect(result.stderr).toContain(
			`${INVENTORY[0].name}@${INVENTORY[0].version}: command accepted; observation incomplete`,
		);
		for (const { name, version } of INVENTORY.slice(1))
			expect(result.stderr).toContain(`${name}@${version}: unattempted`);
		expect(result.stderr).toContain("failed phase: post-publish observation");
		expect(result.stderr).toContain("budget 600000ms");
		expect(result.stderr).toContain("final verification: not completed");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after latest-tag polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.tagVisibilityDelays = { [INVENTORY[0].name]: 2 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("candidate reconciliation did not converge within 600000ms");
		expect(result.stderr).toContain("observation failed");
		expect(result.stderr).toContain("five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	}, 10_000);

	it("stops after attestation-metadata polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.attestationDelays = { [INVENTORY[0].name]: 2 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath, [], { POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("candidate reconciliation did not converge within 600000ms");
		expect(result.stderr).toContain("observation failed");
		expect(result.stderr).toContain("five-fresh forward release");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	}, 10_000);

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
		for (const path of state.verificationRoots ?? []) expect(existsSync(path)).toBe(false);
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
			const result = runHelper("verify", statePath, [], { POLL_DELAY_MS: "300000" });
			expect(result.status).not.toBe(0);
			expect(result.stderr).toContain(`${INVENTORY[2].name}@${INVENTORY[2].version} ${message}`);
			expect(result.stderr).toContain(
				`Verification metadata observed: ${INVENTORY.slice(0, 2)
					.map(({ name, version }) => `${name}@${version}`)
					.join(", ")}`,
			);
			expect(result.stderr).not.toContain("publication state is ambiguous");
			expect(result.stderr).toContain("five-fresh forward release");
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
		expect(install).toContain("--json");
		expect(install).toContain("--ignore-scripts=false");
		expect(state.calls).toContainEqual(["audit", "signatures", "--registry", "https://registry.npmjs.org/"]);
		const smokeIndex = state.calls.findIndex(([command]) => command === "installed-runtime-smoke");
		const cliIndex = state.calls.findIndex(([command]) => command === "installed-scramjet");
		expect(smokeIndex).toBeGreaterThan(state.calls.findIndex(([command]) => command === "audit"));
		expect(cliIndex).toBeGreaterThan(smokeIndex);
		expect(state.verificationPaths!.packageRoot).toBe(
			join(state.verificationPaths!.project, "node_modules", "@leanandmean", "scramjet"),
		);
		expect(state.calls).toContainEqual(["installed-scramjet", "--help"]);
		expect(publishCalls(state)).toHaveLength(0);
		expect(result.stdout).toContain("final verification: completed");
		expectVerificationPathsRemoved(state);
	});

	it("recovers final-install failure after complete publication by independently verifying without republishing", () => {
		const first = runHelper("publish", statePath);
		expect(first.status).toBe(0);
		const state = readState(statePath);
		state.installFailure = true;
		writeFileSync(statePath, JSON.stringify(state));
		const failed = runHelper("verify", statePath);
		expect(failed.status).not.toBe(0);
		const afterFailure = readState(statePath);
		expect(publishCalls(afterFailure)).toHaveLength(5);
		delete afterFailure.installFailure;
		afterFailure.calls = [];
		writeFileSync(statePath, JSON.stringify(afterFailure));
		const recovered = runHelper("verify-cli", statePath, [], { GITHUB_RUN_ATTEMPT: "2", IN_PROCESS_NPM: "1" });
		expect(recovered.status).toBe(0);
		expect(recovered.stdout).toContain("release run 36056969151 attempt 2");
		expect(recovered.stdout).toContain("final verification: completed");
		const final = readState(statePath);
		expect(final.calls.some(([command]) => command === "pack" || command === "publish")).toBe(false);
		expect(final.calls.filter(([command]) => command === "install")).toHaveLength(1);
		expect(final.calls.filter(([command]) => command === "audit")).toHaveLength(1);
		expect(final.calls.filter(([command]) => command === "installed-runtime-smoke")).toHaveLength(1);
		expect(final.calls.filter(([command]) => command === "installed-scramjet")).toHaveLength(1);
		expectVerificationPathsRemoved(final);
	});

	it("retries only the exact target tarball E404 with fresh install roots, then verifies once", () => {
		const state = publishedState();
		const { name, version } = INVENTORY[3];
		state.installFailures = [
			{
				code: "E404",
				summary: `404 Not Found - GET https://registry.npmjs.org/${name}/-/coding-agent-${version}.tgz - not found`,
			},
		];
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).toBe(0);
		const final = readState(statePath);
		expect(final.calls.filter(([command]) => command === "install")).toHaveLength(2);
		expect(final.verificationRoots).toHaveLength(2);
		expect(new Set(final.verificationRoots).size).toBe(2);
		expect(final.calls.filter(([command]) => command === "audit")).toHaveLength(1);
		expect(final.calls.filter(([command]) => command === "installed-runtime-smoke")).toHaveLength(1);
		expect(final.calls.filter(([command]) => command === "installed-scramjet")).toHaveLength(1);
		expect(publishCalls(final)).toHaveLength(0);
		expectVerificationPathsRemoved(final);
	});

	it.each(["E408", "E429", "E500", "ETIMEDOUT", "ECONNRESET", "ECONNREFUSED", "EAI_AGAIN", "ENETUNREACH"])(
		"retries a structured %s install failure",
		(code) => {
			const state = publishedState();
			state.installFailures = [{ code }];
			writeFileSync(statePath, JSON.stringify(state));
			const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
			expect(result.status).toBe(0);
			const final = readState(statePath);
			expect(final.calls.filter(([command]) => command === "install")).toHaveLength(2);
			expectVerificationPathsRemoved(final);
		},
	);

	it.each([
		[
			"other tarball",
			{
				code: "E404",
				summary: "404 Not Found - GET https://registry.npmjs.org/@leanandmean/other/-/other-1.0.0.tgz - not found",
			},
		],
		[
			"near-match tarball",
			{
				code: "E404",
				summary: `404 Not Found - GET https://registry.npmjs.org/${INVENTORY[3].name}/-/coding-agent-${INVENTORY[3].version}.tgz.extra - not found`,
			},
		],
		[
			"missing packument",
			{ code: "E404", summary: `404 Not Found - GET https://registry.npmjs.org/${INVENTORY[3].name} - not found` },
		],
		["unknown code", { code: "ETARGET" }],
		["integrity failure", { code: "EINTEGRITY" }],
		["script failure", { code: "ELIFECYCLE" }],
	] as const)("does not retry %s", (_label, failure) => {
		const state = publishedState();
		state.installFailures = [failure];
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).not.toBe(0);
		const final = readState(statePath);
		expect(final.calls.filter(([command]) => command === "install")).toHaveLength(1);
		expect(final.calls.some(([command]) => command === "audit")).toBe(false);
		expectVerificationPathsRemoved(final);
	});

	it("falls back to structured stderr when stdout is malformed, but rejects unparsable output", () => {
		const state = publishedState();
		state.installFailures = [
			{ code: "E408", stdout: "not json", stderr: JSON.stringify({ error: { code: "E429" } }) },
		];
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).toBe(0);
		const malformed = readState(statePath);
		malformed.calls = [];
		malformed.installFailures = [{ code: "", stdout: "not json", stderr: "network failure" }];
		writeFileSync(statePath, JSON.stringify(malformed));
		const stopped = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(stopped.status).not.toBe(0);
		expect(readState(statePath).calls.filter(([command]) => command === "install")).toHaveLength(1);
	});

	it("prefers a valid stdout error to a conflicting stderr error", () => {
		const state = publishedState();
		state.installFailures = [{ code: "ETARGET", stderr: JSON.stringify({ error: { code: "E408" } }) }];
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).not.toBe(0);
		expect(readState(statePath).calls.filter(([command]) => command === "install")).toHaveLength(1);
	});

	it("clamps install timeouts, includes operation time and rejects a late success", () => {
		const state = publishedState();
		state.installDurationMs = 4_000;
		state.installFailures = [{ code: "E408" }];
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1", POLL_DELAY_MS: "1000" });
		expect(result.status).toBe(0);
		const final = readState(statePath);
		expect(final.installTimeouts).toEqual([600_000, 595_000]);
		expectVerificationPathsRemoved(final);
		final.calls = [];
		final.verificationRoots = [];
		final.installTimeouts = [];
		final.installFailures = [];
		final.installDurationMs = 600_000;
		writeFileSync(statePath, JSON.stringify(final));
		const late = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(late.status).not.toBe(0);
		expect(late.stderr).toContain("install completed after the budget expired");
		const expired = readState(statePath);
		expect(expired.calls.some(([command]) => command === "audit")).toBe(false);
		expectVerificationPathsRemoved(expired);
	});

	it("cleans every failed install root on exhausted visibility without starting consumers", () => {
		const state = publishedState();
		state.installFailures = Array.from({ length: 3 }, () => ({ code: "E408" }));
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1", POLL_DELAY_MS: "300000" });
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("did not converge within 600000ms");
		const final = readState(statePath);
		expect(final.calls.filter(([command]) => command === "install")).toHaveLength(2);
		expect(final.calls.some(([command]) => command === "audit")).toBe(false);
		expectVerificationPathsRemoved(final);
	});

	it("does not retry a transient-shaped signature audit failure", () => {
		const state = publishedState();
		state.auditFailure = "transient";
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath, [], { IN_PROCESS_NPM: "1" });
		expect(result.status).not.toBe(0);
		expect(readState(statePath).calls.filter(([command]) => command === "install")).toHaveLength(1);
	});

	it("completes standalone verification after more than 31 stale metadata observations", () => {
		const state = publishedState();
		state.verificationVisibilityDelays = { [INVENTORY[0].name]: 32 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify-delayed", statePath);
		expect(result.stderr).toBe("");
		expect(result.status).toBe(0);
		const finalState = readState(statePath);
		expect(
			finalState.calls.filter(
				([command, name]) => command === "verification-metadata" && name === INVENTORY[0].name,
			),
		).toHaveLength(33);
		for (const { name } of INVENTORY.slice(1)) {
			expect(finalState.calls).toContainEqual(["verification-metadata", name]);
		}
		expect(finalState.calls.some(([command]) => command === "install")).toBe(true);
		expect(finalState.calls).toContainEqual(["audit", "signatures", "--registry", "https://registry.npmjs.org/"]);
		expect(finalState.calls.some(([command]) => command === "installed-runtime-smoke")).toBe(true);
		expect(finalState.calls).toContainEqual(["installed-scramjet", "--help"]);
		expect(publishCalls(finalState)).toHaveLength(0);
		expect(result.stdout).toContain("final verification: completed");
		expectVerificationPathsRemoved(finalState);
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
		["installed runtime smoke", { runtimeSmokeFailure: true }, "runtime smoke failed", false],
		["CLI", { cliFailure: true }, "installed scramjet --help failed", true],
	] as const)("fails %s verification and removes all owned temporary state", (_label, overrides, message, cliRan) => {
		const state = Object.assign(publishedState(), overrides);
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("verify", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain(message);
		expect(result.stderr).toContain(
			`Verification metadata observed: ${INVENTORY.map(({ name, version }) => `${name}@${version}`).join(", ")}`,
		);
		expect(result.stderr).not.toContain("publication state is ambiguous");
		expect(result.stderr).toContain(`failed phase: ${_label === "CLI" ? "installed CLI probe" : _label}`);
		expect(result.stderr).toContain("final verification: not completed");
		const finalState = readState(statePath);
		expect(finalState.calls.some(([command]) => command === "installed-scramjet")).toBe(cliRan);
		expectVerificationPathsRemoved(finalState);
	});
});

describe("release operation bounds and post-publish polling", () => {
	beforeEach(() => {
		vi.spyOn(console, "log").mockImplementation(() => {});
	});
	afterEach(() => vi.restoreAllMocks());

	it.each([10, 11])("rejects a successful read completing at or past its deadline: %ims", async (completedAt) => {
		let elapsedMs = 0;
		const operation = vi.fn(async () => {
			elapsedMs = completedAt;
			return "too late";
		});
		const sleep = vi.fn();
		await expect(
			pollRead("published package", operation, {
				budgetMs: 10,
				now: () => elapsedMs,
				sleep,
				retryIf: isTransientReadError,
			}),
		).rejects.toThrow(/did not converge within 10ms/);
		expect(operation).toHaveBeenCalledOnce();
		expect(sleep).not.toHaveBeenCalled();
	});
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
				{ ...INVENTORY[0], tarballPath: join(tmpdir(), "checked-candidate.tgz") },
				(...args: any[]) => {
					calls.push(args);
					throw failure;
				},
				25,
			);
		} catch (error) {
			thrown = error as Error;
		}
		expect(thrown?.message).toContain(`npm publish for ${INVENTORY[0].name}@${INVENTORY[0].version} failed`);
		expect((thrown as Error & { cause?: unknown }).cause).toBe(failure);
		expect(calls).toHaveLength(1);
		expect(calls[0][2]).toMatchObject({ timeout: 25, encoding: "utf8" });
	});

	it("uses the production polling interval by default", async () => {
		let attempts = 0;
		let elapsedMs = 0;
		const sleep = vi.fn(async (duration: number) => {
			elapsedMs += duration;
		});
		await pollRead(
			"published package",
			async () => {
				attempts += 1;
				if (attempts === 1) throw new Error("not ready");
			},
			{ now: () => elapsedMs, sleep },
		);
		expect(sleep).toHaveBeenCalledOnce();
		expect(sleep).toHaveBeenCalledWith(10_000);
	});

	it("tolerates delayed registry visibility within one elapsed budget", async () => {
		let attempts = 0;
		let elapsedMs = 0;
		const result = await pollRead(
			"published package",
			async () => {
				attempts += 1;
				if (attempts === 1) throw Object.assign(new Error("timed out"), { code: "ETIMEDOUT" });
				if (attempts === 2) throw new DOMException("fetch timed out", "TimeoutError");
				return "verified";
			},
			{
				budgetMs: 30,
				delayMs: 10,
				now: () => elapsedMs,
				sleep: async (duration) => {
					elapsedMs += duration;
				},
				retryIf: isTransientReadError,
			},
		);
		expect(result).toBe("verified");
		expect(attempts).toBe(3);
	});

	it("permits more than 31 observations within the elapsed budget", async () => {
		let attempts = 0;
		let elapsedMs = 0;
		await pollRead(
			"published package",
			async () => {
				attempts += 1;
				if (attempts <= 32) throw Object.assign(new Error("not visible"), { code: "ETIMEDOUT" });
			},
			{
				budgetMs: 600_000,
				delayMs: 10_000,
				now: () => elapsedMs,
				sleep: async (duration) => {
					elapsedMs += duration;
				},
				retryIf: isTransientReadError,
			},
		);
		expect(attempts).toBe(33);
		expect(elapsedMs).toBe(320_000);
	});

	it("does not start another read after an operation consumes the budget", async () => {
		let elapsedMs = 0;
		const reads: string[] = [];
		await expect(
			pollRead(
				"published package",
				async ({ remainingMs }) => {
					reads.push("versions");
					elapsedMs = 10;
					if (remainingMs() <= 0) throw new DOMException("budget expired", "TimeoutError");
					reads.push("dist-tags");
				},
				{ budgetMs: 10, now: () => elapsedMs, sleep: async () => {}, retryIf: isTransientReadError },
			),
		).rejects.toThrow(/after 10ms and 1 observations: budget expired/);
		expect(reads).toEqual(["versions"]);
	});

	it("clamps sleep and operation context to the remaining budget", async () => {
		let elapsedMs = 0;
		const remaining: number[] = [];
		const sleeps: number[] = [];
		await expect(
			pollRead(
				"published package",
				async ({ remainingMs }) => {
					remaining.push(remainingMs());
					elapsedMs += 6;
					throw new DOMException("still missing", "TimeoutError");
				},
				{
					budgetMs: 25,
					delayMs: 10,
					now: () => elapsedMs,
					sleep: async (duration) => {
						sleeps.push(duration);
						elapsedMs += duration;
					},
					retryIf: isTransientReadError,
				},
			),
		).rejects.toThrow(/within 25ms after 25ms and 2 observations: still missing/);
		expect(remaining).toEqual([25, 9]);
		expect(sleeps).toEqual([10, 3]);
	});
});
