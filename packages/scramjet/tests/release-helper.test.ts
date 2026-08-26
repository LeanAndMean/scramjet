import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
	compareVersions,
	createProvenanceVerifier,
	fetchAttestationJson,
	isTransientReadError,
	loadInventory,
	pollRead,
	publishPackage,
	reconcilePackage,
	run,
	validateIdentity,
} from "../../../.github/scripts/release.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "../../..");
const HELPER = join(REPO_ROOT, ".github", "scripts", "release.mjs");
const RELEASE_GUIDE = readFileSync(join(REPO_ROOT, ".github", "RELEASING.md"), "utf8");
const SHA = execFileSync("git", ["rev-parse", "HEAD"], { cwd: REPO_ROOT, encoding: "utf8" }).trim();
const INVENTORY = loadInventory(REPO_ROOT);
const SCRAMJET_VERSION = INVENTORY.find(({ name }) => name === "@leanandmean/scramjet")!.version;
const RELEASE_ENV = {
	GITHUB_EVENT_NAME: "push",
	GITHUB_REF: `refs/tags/v${SCRAMJET_VERSION}`,
	GITHUB_SHA: SHA,
	GITHUB_WORKFLOW_REF: `LeanAndMean/scramjet/.github/workflows/release.yml@refs/tags/v${SCRAMJET_VERSION}`,
};

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
	badAttestation?: string;
	priorAttestation?: string;
	localPackMismatch?: string;
	versionQueries?: Record<string, number>;
	publicationCounts?: Record<string, number>;
	prePublishLatest?: Record<string, string>;
	visibilityDelays?: Record<string, number>;
	tagVisibilityDelays?: Record<string, number>;
	attestationDelays?: Record<string, number>;
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
  if (field === "dist.integrity") {
    const version = spec.slice(versionSeparator + 1);
    const target = Object.values(state.targets).find((entry) => entry.name === name);
    if (versionSeparator <= 0 || target?.version !== version || !pkg.versions.includes(version)) stop("unknown package version");
    const digest = createHash("sha512").update(name + "@" + version).digest("base64");
    save(); process.stdout.write(JSON.stringify({
      "dist.integrity": "sha512-" + digest,
      "dist.attestations.url": "https://registry.npmjs.org/fake/" + encodeURIComponent(name) + "/" + version
    })); process.exit(0);
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
if (args[0] === "pack") {
  const workspace = args[args.indexOf("-w") + 1];
  const target = state.targets[workspace];
  if (!target) stop("unknown workspace");
  const packDirectory = args[args.indexOf("--pack-destination") + 1];
  const filename = "package.tgz";
  const content = state.localPackMismatch === target.name ? "different package bytes" : target.name + "@" + target.version;
  fs.writeFileSync(require("node:path").join(packDirectory, filename), content);
  save(); process.stdout.write(JSON.stringify([{ filename }])); process.exit(0);
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
stop("unexpected command");
`;

const FAKE_FETCH = `
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
export async function fetchJson(input) {
  const url = new URL(input);
  const parts = url.pathname.slice("/fake/".length).split("/");
  const name = decodeURIComponent(parts[0]);
  const version = parts[1];
  const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
  if ((state.attestationDelays?.[name] ?? 0) > 0) {
    state.attestationDelays[name] -= 1;
    writeFileSync(process.env.FAKE_NPM_STATE, JSON.stringify(state));
    return { attestations: [] };
  }
  const digest = createHash("sha512").update(name + "@" + version).digest("hex");
  const ref = state.priorAttestation === name ? "refs/tags/v0.80.0" : process.env.GITHUB_REF;
  const sha = state.priorAttestation === name ? "b".repeat(40) : process.env.GITHUB_SHA;
  const statement = {
    _type: "https://in-toto.io/Statement/v1",
    subject: [{ name: "pkg:npm/" + name.replace("@", "%40") + "@" + version, digest: { sha512: state.badAttestation === name ? "00" : digest } }],
    predicateType: "https://slsa.dev/provenance/v1",
    predicate: { buildDefinition: {
      externalParameters: { workflow: {
        ref,
        repository: "https://github.com/LeanAndMean/scramjet",
        path: ".github/workflows/release.yml"
      } },
      resolvedDependencies: [{
        uri: "git+https://github.com/LeanAndMean/scramjet@" + ref,
        digest: { gitCommit: sha }
      }]
    } }
  };
  return { attestations: [{
    predicateType: "https://slsa.dev/provenance/v1",
    bundle: {
      verificationMaterial: { testAuthenticated: true },
      dsseEnvelope: {
        payloadType: "application/vnd.in-toto+json",
        payload: Buffer.from(JSON.stringify(statement)).toString("base64")
      }
    }
  }] };
}
export async function verifyBundle(bundle) {
  if (bundle?.verificationMaterial?.testAuthenticated !== true) throw new Error("test bundle was not authenticated");
}
`;

function previousVersion(version: string): string {
	const runtime = /^(\d+\.\d+\.\d+-scramjet\.)(\d+)$/.exec(version);
	if (runtime) return `${runtime[1]}${Number(runtime[2]) - 1}`;
	const stable = /^(\d+)\.(\d+)\.(\d+)$/.exec(version)!;
	return `${stable[1]}.${stable[2]}.${Number(stable[3]) - 1}`;
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

function runHelper(mode: string, statePath: string) {
	const script = ["publish", "reconcile"].includes(mode) ? join(dirname(statePath), "runner.mjs") : HELPER;
	return spawnSync(process.execPath, [script, mode], {
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

	it("accepts only an aligned push event, tag, workflow ref, SHA, and HEAD", () => {
		expect(validateIdentity(INVENTORY, RELEASE_ENV, () => SHA)).toEqual({ ref: RELEASE_ENV.GITHUB_REF, sha: SHA });
	});

	it("validates a clean checkout before dependencies are installed", () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-validate-"));
		try {
			mkdirSync(join(root, ".github", "scripts"), { recursive: true });
			writeFileSync(join(root, ".github", "scripts", "release.mjs"), readFileSync(HELPER));
			for (const { workspace } of INVENTORY) {
				mkdirSync(join(root, workspace), { recursive: true });
				writeFileSync(
					join(root, workspace, "package.json"),
					readFileSync(join(REPO_ROOT, workspace, "package.json")),
				);
			}
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

	it("executes the documented local reconciliation procedure read-only", () => {
		const block = RELEASE_GUIDE.match(/```bash\n(set -euo pipefail[\s\S]*?)\n```/)?.[1];
		expect(block).toBeDefined();
		expect(block).not.toMatch(/release\.mjs publish|gh release create|git (?:push|tag)/);
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-recovery-"));
		try {
			const remote = join(root, "remote.git");
			const source = join(root, "source");
			const checkout = join(root, "checkout");
			mkdirSync(source);
			const git = (cwd: string, args: string[]) =>
				spawnSync("git", args, { cwd, encoding: "utf8", env: { ...process.env, GIT_CONFIG_NOSYSTEM: "1" } });
			expect(git(root, ["init", "--bare", "--quiet", remote]).status).toBe(0);
			expect(git(source, ["init", "--quiet"]).status).toBe(0);
			expect(git(source, ["config", "user.name", "Release Test"]).status).toBe(0);
			expect(git(source, ["config", "user.email", "release-test@example.test"]).status).toBe(0);
			mkdirSync(join(source, "packages", "scramjet"), { recursive: true });
			writeFileSync(join(source, "packages", "scramjet", "package.json"), '{"version":"1.2.3"}\n');
			expect(git(source, ["add", "."]).status).toBe(0);
			expect(git(source, ["commit", "--quiet", "-m", "release fixture"]).status).toBe(0);
			expect(git(source, ["tag", "v1.2.3"]).status).toBe(0);
			expect(git(source, ["remote", "add", "origin", remote]).status).toBe(0);
			expect(git(source, ["push", "--quiet", "origin", "HEAD", "refs/tags/v1.2.3"]).status).toBe(0);
			expect(git(root, ["clone", "--quiet", remote, checkout]).status).toBe(0);
			const tagSha = git(source, ["rev-parse", "v1.2.3"]).stdout.trim();
			const bin = join(root, "bin");
			const record = join(root, "record");
			mkdirSync(bin);
			writeFileSync(
				join(bin, "node"),
				`#!/bin/sh
if test "$1" = "-p"; then printf '1.2.3\\n'; exit 0; fi
printf 'node|%s|%s|%s|%s|%s\\n' "$GITHUB_EVENT_NAME" "$GITHUB_REF" "$GITHUB_SHA" "$GITHUB_WORKFLOW_REF" "$*" >> "$RECOVERY_RECORD"
`,
			);
			writeFileSync(join(bin, "npm"), `#!/bin/sh\nprintf 'npm|%s\\n' "$*" >> "$RECOVERY_RECORD"\n`);
			chmodSync(join(bin, "node"), 0o755);
			chmodSync(join(bin, "npm"), 0o755);
			const result = spawnSync("bash", ["-c", block!.replace("TAG=v0.0.0", "TAG=v1.2.3")], {
				cwd: checkout,
				encoding: "utf8",
				env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, RECOVERY_RECORD: record },
			});
			expect(result.status).toBe(0);
			expect(readFileSync(record, "utf8")).toBe(
				`npm|ci --ignore-scripts\nnpm|run build\nnode|push|refs/tags/v1.2.3|${tagSha}|LeanAndMean/scramjet/.github/workflows/release.yml@refs/tags/v1.2.3|.github/scripts/release.mjs reconcile\n`,
			);
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
		writeFileSync(join(workDir, "fake-fetch.mjs"), FAKE_FETCH);
		writeFileSync(
			join(workDir, "runner.mjs"),
			`import { loadInventory, publish, reconcile, validateIdentity } from ${JSON.stringify(pathToFileURL(HELPER).href)};
import { fetchJson, verifyBundle } from "./fake-fetch.mjs";
try {
  const inventory = loadInventory();
  const identity = validateIdentity(inventory);
  const operation = process.argv[2] === "publish" ? publish : reconcile;
  await operation(inventory, identity, {
    fetchJson,
    verifyBundle,
    pollDependencies: { delayMs: 0, sleep: async () => {} }
  });
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

	it("preflights all five packages without publishing", () => {
		const result = runHelper("preflight", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout.match(/: missing;/g)).toHaveLength(5);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("runs local event-shaped reconciliation without publishing missing packages", () => {
		const result = runHelper("reconcile", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout.match(/^Not yet published /gm)).toHaveLength(5);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
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

	it("reconciles and skips an already-present version", () => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version);
		state.packages[present.name].distTags.latest = present.version;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`Skipping reconciled ${present.name}@${present.version}`);
		expect(publishCalls(readState(statePath)).map((args) => args[args.indexOf("-w") + 1])).toEqual(
			INVENTORY.slice(1).map(({ workspace }) => workspace),
		);
	});

	it("accepts an unchanged package from a prior trusted Scramjet release", () => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version);
		state.packages[present.name].distTags.latest = present.version;
		state.priorAttestation = present.name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`Skipping reconciled ${present.name}@${present.version}`);
		expect(publishCalls(readState(statePath)).map((args) => args[args.indexOf("-w") + 1])).toEqual(
			INVENTORY.slice(1).map(({ workspace }) => workspace),
		);
	});

	it("rejects prior provenance when current packed bytes differ from the registry artifact", () => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version);
		state.packages[present.name].distTags.latest = present.version;
		state.priorAttestation = present.name;
		state.localPackMismatch = present.name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("prior-release artifact does not match");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("reconciles a mixed partial release without requiring missing packages", () => {
		const state = initialState();
		for (const present of INVENTORY.slice(0, 2)) {
			state.packages[present.name].versions.push(present.version);
			state.packages[present.name].distTags.latest = present.version;
		}
		state.priorAttestation = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("reconcile", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout.match(/^Reconciled /gm)).toHaveLength(2);
		expect(result.stdout.match(/^Not yet published /gm)).toHaveLength(3);
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("rejects a present target that is not current latest", () => {
		const state = initialState();
		const present = INVENTORY[0];
		state.packages[present.name].versions.push(present.version, "999.0.0");
		state.packages[present.name].distTags.latest = "999.0.0";
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("is present but is not current latest");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("reconciles every present version before the first publication", () => {
		const state = initialState();
		const present = INVENTORY.at(-1)!;
		state.packages[present.name].versions.push(present.version);
		state.packages[present.name].distTags.latest = present.version;
		state.badAttestation = present.name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("digest does not match");
		expect(publishCalls(readState(statePath))).toHaveLength(0);
	});

	it("reconciles a new publication before continuing", () => {
		const state = initialState();
		state.badAttestation = INVENTORY[0].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("digest does not match");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
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

	it("treats every publish failure as ambiguous without retrying", () => {
		const state = initialState();
		state.publishFailure = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
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
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("polls delayed version, tag, and attestation visibility through publication", () => {
		const state = initialState();
		const first = INVENTORY[0].name;
		state.visibilityDelays = { [first]: 1 };
		state.tagVisibilityDelays = { [first]: 1 };
		state.attestationDelays = { [first]: 1 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).toBe(0);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} registry visibility not ready`);
		expect(result.stdout).toContain(`${first}@${INVENTORY[0].version} provenance not ready`);
		const finalState = readState(statePath);
		expect(finalState.publicationCounts?.[first]).toBe(1);
		expect(publishCalls(finalState)).toHaveLength(INVENTORY.length);
	});

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
		state.visibilityDelays = { [INVENTORY[0].name]: 6 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("registry visibility did not converge after 6 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after latest-tag polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.tagVisibilityDelays = { [INVENTORY[0].name]: 6 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("registry visibility did not converge after 6 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});

	it("stops after provenance polling is exhausted without republishing or continuing", () => {
		const state = initialState();
		state.attestationDelays = { [INVENTORY[0].name]: 6 };
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
		expect(result.stderr).toContain("provenance did not converge after 6 attempts");
		expect(result.stderr).toContain("publication state is ambiguous");
		expect(result.stderr).toContain("Do not retry publication; run read-only reconciliation");
		expect(publishCalls(readState(statePath))).toHaveLength(1);
	});
});

function provenanceFixture() {
	const pkg = { name: "@leanandmean/scramjet", version: "1.2.3" };
	const sha = "a".repeat(40);
	const ref = "refs/tags/v1.2.3";
	const tarball = Buffer.from("fixture tarball");
	const digest = createHash("sha512").update(tarball).digest();
	const statement = {
		_type: "https://in-toto.io/Statement/v1",
		subject: [{ name: "pkg:npm/%40leanandmean/scramjet@1.2.3", digest: { sha512: digest.toString("hex") } }],
		predicateType: "https://slsa.dev/provenance/v1",
		predicate: {
			buildDefinition: {
				externalParameters: {
					workflow: {
						ref,
						repository: "https://github.com/LeanAndMean/scramjet",
						path: ".github/workflows/release.yml",
					},
				},
				resolvedDependencies: [
					{
						uri: `git+https://github.com/LeanAndMean/scramjet@${ref}`,
						digest: { gitCommit: sha },
					},
				],
			},
		},
	};
	const response = {
		attestations: [
			{
				predicateType: "https://slsa.dev/provenance/v1",
				bundle: {
					dsseEnvelope: {
						payloadType: "application/vnd.in-toto+json",
						payload: Buffer.from(JSON.stringify(statement)).toString("base64"),
					},
				},
			},
		],
	};
	return {
		pkg,
		identity: { ref, sha },
		metadata: {
			"dist.integrity": `sha512-${digest.toString("base64")}`,
			"dist.attestations.url": "https://registry.npmjs.org/fixture",
		},
		statement,
		response,
	};
}

function updatePayload(fixture: ReturnType<typeof provenanceFixture>) {
	fixture.response.attestations[0].bundle.dsseEnvelope.payload = Buffer.from(
		JSON.stringify(fixture.statement),
	).toString("base64");
}

function reconcileFixture(fixture: ReturnType<typeof provenanceFixture>, dependencies = {}) {
	return reconcilePackage(fixture.pkg, fixture.identity, {
		registry: () => fixture.metadata,
		fetchJson: async () => fixture.response,
		verifyBundle: async () => {},
		...dependencies,
	});
}

describe("release helper provenance reconciliation", () => {
	it("configures Sigstore for the exact GitHub Actions trust boundary", async () => {
		const verify = vi.fn();
		const createVerifier = vi.fn(async () => ({ verify }));
		const authenticate = await createProvenanceVerifier(createVerifier);
		const bundle = { mediaType: "application/vnd.dev.sigstore.bundle.v0.3+json" };
		await authenticate(bundle);
		expect(createVerifier).toHaveBeenCalledWith({
			certificateIssuer: "https://token.actions.githubusercontent.com",
			certificateIdentityURI:
				"^https://github\\.com/LeanAndMean/scramjet/\\.github/workflows/release\\.yml@refs/tags/v\\d+\\.\\d+\\.\\d+$",
			ctLogThreshold: 1,
			tlogThreshold: 1,
			timeout: 60_000,
			retry: 0,
		});
		expect(verify).toHaveBeenCalledWith(bundle);
	});

	it.each([
		["an unsigned bundle", "no signatures"],
		["an invalid signature", "signature verification failed"],
		["the wrong certificate identity", "certificate identity mismatch"],
		["missing transparency evidence", "transparency log threshold not met"],
	])("rejects %s before parsing claims", async (_label, message) => {
		const fixture = provenanceFixture();
		fixture.response.attestations[0].bundle.dsseEnvelope.payload = "not-json";
		const authenticate = await createProvenanceVerifier(async () => ({
			verify: async () => {
				throw new Error(message);
			},
		}));
		await expect(reconcileFixture(fixture, { verifyBundle: authenticate })).rejects.toThrow(
			`Sigstore provenance authentication failed: ${message}`,
		);
	});

	it("accepts authenticated exact package, digest, workflow, tag, source, and commit evidence", async () => {
		const fixture = provenanceFixture();
		await expect(reconcileFixture(fixture)).resolves.toBe(true);
	});

	it.each([
		[
			"missing statement type",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				delete (fixture.statement as { _type?: string })._type;
			},
		],
		[
			"wrong statement type",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement._type = "https://in-toto.io/Statement/v0.1";
			},
		],
		[
			"package subject",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.subject[0].name = "pkg:npm/other@1.2.3";
			},
		],
		[
			"digest",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.subject[0].digest.sha512 = "00";
			},
		],
		[
			"workflow repository",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.predicate.buildDefinition.externalParameters.workflow.repository =
					"https://github.com/other/repo";
			},
		],
		[
			"workflow path",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.predicate.buildDefinition.externalParameters.workflow.path =
					".github/workflows/other.yml";
			},
		],
		[
			"workflow ref",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.predicate.buildDefinition.externalParameters.workflow.ref = "refs/heads/main";
			},
		],
		[
			"resolved source",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.predicate.buildDefinition.resolvedDependencies[0].uri =
					"git+https://github.com/LeanAndMean/scramjet@refs/heads/main";
			},
		],
		[
			"resolved commit",
			(fixture: ReturnType<typeof provenanceFixture>) => {
				fixture.statement.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit = "b".repeat(40);
			},
		],
	])("rejects a mismatched %s", async (_label, mutate) => {
		const fixture = provenanceFixture();
		mutate(fixture);
		updatePayload(fixture);
		await expect(reconcileFixture(fixture)).rejects.toThrow();
	});

	it("rejects an empty SLSA provenance response", async () => {
		const fixture = provenanceFixture();
		fixture.response.attestations = [];
		await expect(reconcileFixture(fixture)).rejects.toThrow(/an SLSA provenance statement/);
	});

	it("rejects multiple SLSA provenance statements without retrying", async () => {
		const fixture = provenanceFixture();
		fixture.response.attestations.push(structuredClone(fixture.response.attestations[0]));
		let attempts = 0;
		await expect(
			pollRead(
				"provenance",
				async () => {
					attempts += 1;
					return reconcileFixture(fixture);
				},
				{ attempts: 3, delayMs: 0, sleep: async () => {}, retryIf: isTransientReadError },
			),
		).rejects.toThrow(/exactly one SLSA provenance statement/);
		expect(attempts).toBe(1);
	});

	it("requires exactly one package subject", async () => {
		const fixture = provenanceFixture();
		fixture.statement.subject.push(structuredClone(fixture.statement.subject[0]));
		updatePayload(fixture);
		await expect(reconcileFixture(fixture)).rejects.toThrow(/exactly one subject/);
	});

	it.each([
		["invalid alphabet", "sha512-!"],
		["short digest", "sha512-QQ=="],
		["noncanonical padding", `sha512-${Buffer.alloc(64).toString("base64").replace(/==$/, "")}`],
	])("rejects %s in registry integrity", async (_label, integrity) => {
		const fixture = provenanceFixture();
		fixture.metadata["dist.integrity"] = integrity;
		await expect(reconcileFixture(fixture)).rejects.toThrow(/integrity|SHA-512/);
	});

	it("rejects an off-registry attestation URL before fetching", async () => {
		const fixture = provenanceFixture();
		fixture.metadata["dist.attestations.url"] = "https://example.test/attestations";
		await expect(
			reconcilePackage(fixture.pkg, fixture.identity, { registry: () => fixture.metadata }),
		).rejects.toThrow(/npm registry over HTTPS/);
	});

	it.each([
		["malformed prior tag", "refs/tags/v1.2", "b".repeat(40)],
		["non-release prior ref", "refs/heads/main", "b".repeat(40)],
		["malformed prior SHA", "refs/tags/v1.2.2", "not-a-sha"],
	])("rejects a prior release with a %s", async (_label, ref, sha) => {
		const fixture = provenanceFixture();
		fixture.statement.predicate.buildDefinition.externalParameters.workflow.ref = ref;
		fixture.statement.predicate.buildDefinition.resolvedDependencies[0].uri = `git+https://github.com/LeanAndMean/scramjet@${ref}`;
		fixture.statement.predicate.buildDefinition.resolvedDependencies[0].digest.gitCommit = sha;
		updatePayload(fixture);
		await expect(reconcileFixture(fixture, { allowPriorRelease: true })).rejects.toThrow();
	});
});

describe("release operation bounds and post-publish polling", () => {
	it("terminates a bounded external read", () => {
		expect(() => run(process.execPath, ["-e", "setTimeout(() => {}, 1000)"], { timeout: 10 })).toThrow();
	});

	it("aborts a bounded attestation fetch", async () => {
		let signal: AbortSignal | undefined;
		await expect(
			fetchAttestationJson(
				"https://registry.npmjs.org/fixture",
				async (_url: URL, options: { signal: AbortSignal }) => {
					signal = options.signal;
					return new Promise((_resolve, reject) => {
						options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true });
					});
				},
				10,
			),
		).rejects.toThrow();
		expect(signal?.aborted).toBe(true);
	});

	it.each([400, 401, 403])("fails immediately for permanent attestation HTTP %s", async (status) => {
		let fetchAttempts = 0;
		let pollAttempts = 0;
		await expect(
			pollRead(
				"provenance",
				async () => {
					pollAttempts += 1;
					return fetchAttestationJson("https://registry.npmjs.org/fixture", async () => {
						fetchAttempts += 1;
						return { ok: false, status } as Response;
					});
				},
				{ attempts: 3, delayMs: 0, sleep: async () => {}, retryIf: isTransientReadError },
			),
		).rejects.toThrow(`attestation request failed with HTTP ${status} for https://registry.npmjs.org/fixture`);
		expect(fetchAttempts).toBe(1);
		expect(pollAttempts).toBe(1);
	});

	it.each([404, 408, 429, 500, 503])("retries transient attestation HTTP %s within the bound", async (status) => {
		let attempts = 0;
		await expect(
			pollRead(
				"provenance",
				async () => {
					attempts += 1;
					return fetchAttestationJson("https://registry.npmjs.org/fixture", async () =>
						attempts < 3
							? ({ ok: false, status } as Response)
							: ({ ok: true, json: async () => ({}) } as Response),
					);
				},
				{ attempts: 3, delayMs: 0, sleep: async () => {}, retryIf: isTransientReadError },
			),
		).resolves.toEqual({});
		expect(attempts).toBe(3);
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
		expect(thrown?.message).toMatch(/state is ambiguous.*Do not retry publication.*read-only reconciliation/);
		expect((thrown as Error & { cause?: unknown }).cause).toBe(failure);
		expect(calls).toHaveLength(1);
		expect(calls[0][2]).toMatchObject({ timeout: 25 });
	});

	it("tolerates delayed registry and attestation visibility", async () => {
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
