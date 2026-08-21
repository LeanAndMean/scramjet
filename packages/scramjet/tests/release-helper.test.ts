import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
	compareVersions,
	loadInventory,
	reconcilePackage,
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
};

interface FakeState {
	packages: Record<string, { versions: string[]; distTags: Record<string, string> }>;
	targets: Record<string, { name: string; version: string }>;
	calls: string[][];
	failure?: { name: string; field: string; output?: string; status?: number };
	publishFailure?: string;
	race?: string;
	unexpectedTagChange?: string;
	badAttestation?: string;
	versionQueries?: Record<string, number>;
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
    if (state.failure.status) { console.error(state.failure.output ?? "registry failed"); process.exit(state.failure.status); }
    process.stdout.write(state.failure.output ?? "not json"); process.exit(0);
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
    if (state.race === name && state.versionQueries[name] === 2) pkg.versions.push(state.targets[Object.keys(state.targets).find((key) => state.targets[key].name === name)].version);
    save(); process.stdout.write(JSON.stringify(pkg.versions)); process.exit(0);
  }
  if (field === "dist-tags") { save(); process.stdout.write(JSON.stringify(pkg.distTags)); process.exit(0); }
  stop("unexpected view");
}
if (args[0] === "publish") {
  const workspace = args[args.indexOf("-w") + 1];
  const target = state.targets[workspace];
  if (!target) stop("unknown workspace");
  if (state.publishFailure === target.name) stop("publish failed");
  const pkg = state.packages[target.name];
  pkg.versions.push(target.version);
  pkg.distTags.latest = target.version;
  if (state.unexpectedTagChange === target.name) pkg.distTags.scramjet = target.version;
  save(); process.stdout.write("published"); process.exit(0);
}
stop("unexpected command");
`;

const FAKE_FETCH = `
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
globalThis.fetch = async (input) => {
  const url = new URL(input);
  const parts = url.pathname.slice("/fake/".length).split("/");
  const name = decodeURIComponent(parts[0]);
  const version = parts[1];
  const state = JSON.parse(readFileSync(process.env.FAKE_NPM_STATE, "utf8"));
  const digest = createHash("sha512").update(name + "@" + version).digest("hex");
  const statement = {
    _type: "https://in-toto.io/Statement/v1",
    subject: [{ name: "pkg:npm/" + name.replace("@", "%40") + "@" + version, digest: { sha512: state.badAttestation === name ? "00" : digest } }],
    predicateType: "https://slsa.dev/provenance/v1",
    predicate: { buildDefinition: {
      externalParameters: { workflow: {
        ref: process.env.GITHUB_REF,
        repository: "https://github.com/LeanAndMean/scramjet",
        path: ".github/workflows/release.yml"
      } },
      resolvedDependencies: [{
        uri: "git+https://github.com/LeanAndMean/scramjet@" + process.env.GITHUB_REF,
        digest: { gitCommit: process.env.GITHUB_SHA }
      }]
    } }
  };
  const response = { attestations: [{
    predicateType: "https://slsa.dev/provenance/v1",
    bundle: { dsseEnvelope: {
      payloadType: "application/vnd.in-toto+json",
      payload: Buffer.from(JSON.stringify(statement)).toString("base64")
    } }
  }] };
  return { ok: true, status: 200, json: async () => response };
};
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
	return spawnSync(process.execPath, [HELPER, mode], {
		cwd: REPO_ROOT,
		encoding: "utf8",
		env: {
			...process.env,
			...RELEASE_ENV,
			PATH: `${dirname(statePath)}:${process.env.PATH}`,
			FAKE_NPM_STATE: statePath,
			NODE_OPTIONS: `--import=${join(dirname(statePath), "fake-fetch.mjs")}`,
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
			(manifest: Record<string, unknown>) => {
				manifest.name = "@leanandmean/ai";
			},
		],
		[
			"repository metadata",
			(manifest: Record<string, unknown>) => {
				manifest.repository = "github:LeanAndMean/scramjet";
			},
		],
	])("rejects incorrect %s", (_label, mutate) => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-release-manifests-"));
		try {
			for (const { workspace } of INVENTORY) {
				mkdirSync(join(root, workspace), { recursive: true });
				const manifest = JSON.parse(readFileSync(join(REPO_ROOT, workspace, "package.json"), "utf8"));
				if (workspace === "packages/agent") mutate(manifest);
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
	it("orders stable and Scramjet runtime versions", () => {
		expect(compareVersions("1.2.3", "1.2.2")).toBe(1);
		expect(compareVersions("0.74.1-scramjet.18", "0.74.1-scramjet.17")).toBe(1);
		expect(compareVersions("0.74.1", "0.74.1-scramjet.18")).toBe(1);
	});

	it.each(["1.2", "1.2.3-beta.1", "v1.2.3", "0.74.1-scramjet.x"])("rejects unknown version form %s", (version) => {
		expect(() => compareVersions(version, "1.2.3")).toThrow(/unsupported version/);
	});
});

describe("release helper registry preflight and publication", () => {
	let workDir: string;
	let statePath: string;

	beforeEach(() => {
		workDir = mkdtempSync(join(tmpdir(), "scramjet-release-"));
		writeFileSync(join(workDir, "npm"), FAKE_NPM);
		chmodSync(join(workDir, "npm"), 0o755);
		writeFileSync(join(workDir, "fake-fetch.mjs"), FAKE_FETCH);
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

	it("treats a publish failure as fatal without retrying", () => {
		const state = initialState();
		state.publishFailure = INVENTORY[1].name;
		writeFileSync(statePath, JSON.stringify(state));
		const result = runHelper("publish", statePath);
		expect(result.status).not.toBe(0);
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

describe("release helper provenance reconciliation", () => {
	it("accepts exact package, digest, workflow, tag, source, and commit evidence", async () => {
		const fixture = provenanceFixture();
		await expect(
			reconcilePackage(fixture.pkg, fixture.identity, {
				registry: () => fixture.metadata,
				fetchJson: async () => fixture.response,
			}),
		).resolves.toBe(true);
	});

	it.each([
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
		await expect(
			reconcilePackage(fixture.pkg, fixture.identity, {
				registry: () => fixture.metadata,
				fetchJson: async () => fixture.response,
			}),
		).rejects.toThrow();
	});

	it("requires exactly one package subject and one SLSA statement", async () => {
		const fixture = provenanceFixture();
		fixture.statement.subject.push(structuredClone(fixture.statement.subject[0]));
		updatePayload(fixture);
		await expect(
			reconcilePackage(fixture.pkg, fixture.identity, {
				registry: () => fixture.metadata,
				fetchJson: async () => fixture.response,
			}),
		).rejects.toThrow(/exactly one subject/);
	});
});
