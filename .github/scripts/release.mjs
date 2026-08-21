#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPOSITORY_URL = "git+https://github.com/LeanAndMean/scramjet.git";
const WORKFLOW_REPOSITORY = "https://github.com/LeanAndMean/scramjet";
const WORKFLOW_PATH = ".github/workflows/release.yml";
const SLSA_PREDICATE = "https://slsa.dev/provenance/v1";
const INVENTORY = [
	["packages/tui", "@leanandmean/tui"],
	["packages/ai", "@leanandmean/ai"],
	["packages/agent", "@leanandmean/agent"],
	["packages/coding-agent", "@leanandmean/coding-agent"],
	["packages/scramjet", "@leanandmean/scramjet"],
];
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function fail(message) {
	throw new Error(message);
}

function parseJson(value, description) {
	try {
		return JSON.parse(value);
	} catch {
		fail(`${description} was not valid JSON`);
	}
}

function run(command, args, options = {}) {
	const output = execFileSync(command, args, {
		cwd: REPO_ROOT,
		encoding: "utf8",
		stdio: ["ignore", "pipe", "pipe"],
		...options,
	});
	return typeof output === "string" ? output.trim() : "";
}

export function loadInventory(root = REPO_ROOT) {
	return INVENTORY.map(([workspace, expectedName]) => {
		const manifest = parseJson(readFileSync(join(root, workspace, "package.json"), "utf8"), `${workspace}/package.json`);
		if (manifest.name !== expectedName) fail(`${workspace} must be named ${expectedName}`);
		if (typeof manifest.version !== "string" || manifest.version.length === 0) fail(`${expectedName} has no version`);
		if (
			manifest.repository?.type !== "git" ||
			manifest.repository?.url !== REPOSITORY_URL ||
			Object.keys(manifest.repository).length !== 2
		) {
			fail(`${expectedName} must declare the canonical repository metadata`);
		}
		return { workspace, name: expectedName, version: manifest.version };
	});
}

export function validateIdentity(inventory, env = process.env, git = (args) => run("git", args)) {
	if (env.GITHUB_EVENT_NAME !== "push") fail("GITHUB_EVENT_NAME must be push");
	const scramjet = inventory.find(({ name }) => name === "@leanandmean/scramjet");
	const expectedRef = `refs/tags/v${scramjet.version}`;
	if (env.GITHUB_REF !== expectedRef) fail(`GITHUB_REF must be ${expectedRef}`);
	if (!/^[0-9a-f]{40}$/.test(env.GITHUB_SHA ?? "")) fail("GITHUB_SHA must be a 40-character commit SHA");
	const expectedWorkflowRef = `LeanAndMean/scramjet/${WORKFLOW_PATH}@${expectedRef}`;
	if (env.GITHUB_WORKFLOW_REF !== expectedWorkflowRef) fail(`GITHUB_WORKFLOW_REF must be ${expectedWorkflowRef}`);
	const head = git(["rev-parse", "HEAD"]);
	if (head !== env.GITHUB_SHA) fail("checked-out HEAD must equal GITHUB_SHA");
	return { ref: expectedRef, sha: env.GITHUB_SHA };
}

function npmJson(args, description) {
	return parseJson(run("npm", args), description);
}

function requireVersions(value, name) {
	if (!Array.isArray(value) || value.some((version) => typeof version !== "string")) {
		fail(`${name} versions must be a JSON array containing only strings`);
	}
	return value;
}

function requireDistTags(value, name) {
	if (value === null || Array.isArray(value) || typeof value !== "object") fail(`${name} dist-tags must be a JSON object`);
	if (Object.values(value).some((version) => typeof version !== "string")) {
		fail(`${name} dist-tags must contain only string values`);
	}
	return value;
}

function parseVersion(version) {
	const match = /^(\d+)\.(\d+)\.(\d+)(?:-scramjet\.(\d+))?$/.exec(version);
	if (!match) fail(`unsupported version format: ${version}`);
	return {
		core: match.slice(1, 4).map(Number),
		prerelease: match[4] === undefined ? null : Number(match[4]),
	};
}

export function compareVersions(left, right) {
	const a = parseVersion(left);
	const b = parseVersion(right);
	for (let index = 0; index < a.core.length; index += 1) {
		if (a.core[index] !== b.core[index]) return a.core[index] < b.core[index] ? -1 : 1;
	}
	if (a.prerelease === b.prerelease) return 0;
	if (a.prerelease === null) return 1;
	if (b.prerelease === null) return -1;
	return a.prerelease < b.prerelease ? -1 : 1;
}

export function preflight(inventory) {
	const plan = inventory.map((pkg) => {
		const versions = requireVersions(npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions`), pkg.name);
		const distTags = requireDistTags(npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags`), pkg.name);
		const present = versions.includes(pkg.version);
		if (!present) {
			if (typeof distTags.latest !== "string") fail(`${pkg.name} has no string-valued latest dist-tag`);
			if (compareVersions(pkg.version, distTags.latest) <= 0) {
				fail(`${pkg.name}@${pkg.version} is not newer than latest ${distTags.latest}`);
			}
		}
		return { ...pkg, present, distTags };
	});
	for (const pkg of plan) {
		console.log(`${pkg.name}@${pkg.version}: ${pkg.present ? "present" : "missing"}; dist-tags=${JSON.stringify(pkg.distTags)}`);
	}
	return plan;
}

function expectedPurl(name, version) {
	return `pkg:npm/${name.replace("@", "%40")}@${version}`;
}

function integrityHex(integrity) {
	const match = /^sha512-([A-Za-z0-9+/]+={0,2})$/.exec(integrity);
	if (!match) fail("registry integrity must be an sha512 SRI value");
	return Buffer.from(match[1], "base64").toString("hex");
}

export async function reconcilePackage(pkg, identity, dependencies = {}) {
	const registry = dependencies.registry ?? ((args, description) => npmJson(args, description));
	const fetchJson = dependencies.fetchJson ?? (async (url) => {
		const parsed = new URL(url);
		if (parsed.protocol !== "https:" || parsed.hostname !== "registry.npmjs.org") {
			fail("attestation URL must use the npm registry over HTTPS");
		}
		const response = await fetch(parsed);
		if (!response.ok) fail(`attestation request failed with HTTP ${response.status}`);
		return response.json();
	});
	const metadata = registry(
		["view", `${pkg.name}@${pkg.version}`, "dist.integrity", "dist.attestations.url", "--json"],
		`${pkg.name}@${pkg.version} provenance metadata`,
	);
	if (metadata === null || Array.isArray(metadata) || typeof metadata !== "object") fail("provenance metadata must be an object");
	if (typeof metadata["dist.integrity"] !== "string" || typeof metadata["dist.attestations.url"] !== "string") {
		fail(`${pkg.name}@${pkg.version} must have integrity and an attestation URL`);
	}
	const response = await fetchJson(metadata["dist.attestations.url"]);
	if (!Array.isArray(response?.attestations)) fail("attestation response must contain an attestations array");
	const candidates = response.attestations.filter((entry) => entry?.predicateType === SLSA_PREDICATE);
	if (candidates.length !== 1) fail("attestation response must contain exactly one SLSA provenance statement");
	const envelope = candidates[0]?.bundle?.dsseEnvelope;
	if (envelope?.payloadType !== "application/vnd.in-toto+json" || typeof envelope.payload !== "string") {
		fail("SLSA provenance must contain an in-toto DSSE payload");
	}
	const statement = parseJson(Buffer.from(envelope.payload, "base64").toString("utf8"), "SLSA provenance payload");
	if (statement.predicateType !== SLSA_PREDICATE) fail("SLSA payload has the wrong predicate type");
	if (!Array.isArray(statement.subject) || statement.subject.length !== 1) fail("SLSA payload must have exactly one subject");
	const subject = statement.subject[0];
	if (subject?.name !== expectedPurl(pkg.name, pkg.version)) fail("SLSA subject does not match the package version");
	if (subject?.digest?.sha512 !== integrityHex(metadata["dist.integrity"])) fail("SLSA subject digest does not match registry integrity");
	const build = statement.predicate?.buildDefinition;
	const workflow = build?.externalParameters?.workflow;
	if (workflow?.repository !== WORKFLOW_REPOSITORY) fail("SLSA workflow repository does not match");
	if (workflow?.path !== WORKFLOW_PATH) fail("SLSA workflow path does not match");
	if (workflow?.ref !== identity.ref) fail("SLSA workflow ref does not match");
	const expectedUri = `git+${WORKFLOW_REPOSITORY}@${identity.ref}`;
	const sources = build?.resolvedDependencies;
	if (!Array.isArray(sources) || sources.length !== 1 || sources[0]?.uri !== expectedUri) {
		fail("SLSA resolved source does not match the release tag");
	}
	if (sources[0]?.digest?.gitCommit !== identity.sha) fail("SLSA resolved commit does not match GITHUB_SHA");
	return true;
}

function tagsEqual(left, right) {
	const leftEntries = Object.entries(left).sort(([a], [b]) => a.localeCompare(b));
	const rightEntries = Object.entries(right).sort(([a], [b]) => a.localeCompare(b));
	return JSON.stringify(leftEntries) === JSON.stringify(rightEntries);
}

export async function publish(inventory, identity) {
	const plan = preflight(inventory);
	for (const pkg of plan.filter(({ present }) => present)) {
		await reconcilePackage(pkg, identity);
		const currentTags = requireDistTags(
			npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags after reconciliation`),
			pkg.name,
		);
		if (!tagsEqual(currentTags, pkg.distTags)) fail(`${pkg.name} dist-tags changed after preflight`);
	}
	for (const pkg of plan) {
		if (pkg.present) {
			console.log(`Skipping reconciled ${pkg.name}@${pkg.version}`);
			continue;
		}
		const currentVersions = requireVersions(
			npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions before publish`),
			pkg.name,
		);
		if (currentVersions.includes(pkg.version)) fail(`${pkg.name}@${pkg.version} appeared after preflight; reconcile before retrying`);
		const currentTags = requireDistTags(
			npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags before publish`),
			pkg.name,
		);
		if (!tagsEqual(currentTags, pkg.distTags)) fail(`${pkg.name} dist-tags changed after preflight`);
		if (typeof currentTags.latest !== "string" || compareVersions(pkg.version, currentTags.latest) <= 0) {
			fail(`${pkg.name}@${pkg.version} is not newer than latest ${currentTags.latest}`);
		}
		run("npm", ["publish", "-w", pkg.workspace, "--access", "public", "--provenance", "--tag", "latest"], {
			stdio: "inherit",
		});
		const publishedVersions = requireVersions(
			npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions after publish`),
			pkg.name,
		);
		if (!publishedVersions.includes(pkg.version)) fail(`${pkg.name}@${pkg.version} was not visible after publish`);
		const publishedTags = requireDistTags(
			npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags after publish`),
			pkg.name,
		);
		if (publishedTags.latest !== pkg.version) fail(`${pkg.name} latest did not move to ${pkg.version}`);
		const beforeNonLatest = { ...pkg.distTags };
		const afterNonLatest = { ...publishedTags };
		delete beforeNonLatest.latest;
		delete afterNonLatest.latest;
		if (!tagsEqual(beforeNonLatest, afterNonLatest)) fail(`${pkg.name} non-latest dist-tags changed during publish`);
		await reconcilePackage(pkg, identity);
	}
}

async function main() {
	const mode = process.argv[2];
	if (!["validate", "preflight", "publish", "reconcile"].includes(mode)) {
		fail("usage: release.mjs <validate|preflight|publish|reconcile>");
	}
	const inventory = loadInventory();
	if (mode === "preflight") {
		preflight(inventory);
		return;
	}
	const identity = validateIdentity(inventory);
	if (mode === "validate") return;
	if (mode === "publish") {
		await publish(inventory, identity);
		return;
	}
	for (const pkg of inventory) await reconcilePackage(pkg, identity);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	main().catch((error) => {
		console.error(`release: ${error.message}`);
		process.exitCode = 1;
	});
}
