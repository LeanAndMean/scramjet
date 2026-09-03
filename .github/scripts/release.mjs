#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPOSITORY_URL = "git+https://github.com/LeanAndMean/scramjet.git";
const REGISTRY_URL = "https://registry.npmjs.org/";
const WORKFLOW_PATH = ".github/workflows/release.yml";
export const READ_TIMEOUT_MS = 60_000;
export const PUBLISH_TIMEOUT_MS = 10 * 60_000;
export const POST_PUBLISH_ATTEMPTS = 31;
export const POST_PUBLISH_DELAY_MS = 10_000;
const INVENTORY = [
	["packages/tui", "@leanandmean/tui"],
	["packages/ai", "@leanandmean/ai"],
	["packages/agent", "@leanandmean/agent"],
	["packages/coding-agent", "@leanandmean/coding-agent"],
	["packages/scramjet", "@leanandmean/scramjet"],
];
const INTERNAL_DEPENDENCIES = new Map([
	["@leanandmean/tui", []],
	["@leanandmean/ai", []],
	["@leanandmean/agent", ["@leanandmean/ai"]],
	["@leanandmean/coding-agent", ["@leanandmean/agent", "@leanandmean/ai", "@leanandmean/tui"]],
	["@leanandmean/scramjet", INVENTORY.slice(0, -1).map(([, name]) => name)],
]);
const DEPENDENCY_SECTIONS = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];
const RELEASE_METADATA_PATHS = [...INVENTORY.map(([workspace]) => `${workspace}/package.json`), "package-lock.json"];
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

export function run(command, args, options = {}) {
	const output = execFileSync(command, args, {
		cwd: REPO_ROOT,
		encoding: "utf8",
		stdio: ["ignore", "pipe", "pipe"],
		timeout: READ_TIMEOUT_MS,
		...options,
	});
	return typeof output === "string" ? output.trim() : "";
}

function requireObject(value, description) {
	if (value === null || Array.isArray(value) || typeof value !== "object") fail(`${description} must be a JSON object`);
	return value;
}

function validateInternalDependencies(record, name, versions, description) {
	const actual = new Map();
	for (const section of DEPENDENCY_SECTIONS) {
		if (record[section] === undefined) continue;
		const dependencies = requireObject(record[section], `${description} ${section}`);
		for (const [dependencyName, version] of Object.entries(dependencies)) {
			if (!versions.has(dependencyName)) continue;
			if (section !== "dependencies") fail(`${description} must declare ${dependencyName} only in dependencies`);
			actual.set(dependencyName, version);
		}
	}
	const expected = INTERNAL_DEPENDENCIES.get(name);
	if (actual.size !== expected.length) fail(`${description} must contain the exact fixed internal dependency set`);
	for (const dependencyName of expected) {
		const version = versions.get(dependencyName);
		if (actual.get(dependencyName) !== version) {
			fail(`${description} must depend on exact ${dependencyName}@${version}`);
		}
	}
}

function parseInventory(readMetadata) {
	const manifests = new Map();
	const inventory = INVENTORY.map(([workspace, expectedName]) => {
		const path = `${workspace}/package.json`;
		const manifest = requireObject(parseJson(readMetadata(path), path), path);
		if (manifest.name !== expectedName) fail(`${workspace} must be named ${expectedName}`);
		if (typeof manifest.version !== "string" || manifest.version.length === 0) fail(`${expectedName} has no version`);
		if (
			manifest.repository?.type !== "git" ||
			manifest.repository?.url !== REPOSITORY_URL ||
			Object.keys(manifest.repository).length !== 2
		) {
			fail(`${expectedName} must declare the canonical repository metadata`);
		}
		if (manifest.publishConfig?.access !== "public" || Object.keys(manifest.publishConfig).length !== 1) {
			fail(`${expectedName} publishConfig must contain only public access`);
		}
		manifests.set(expectedName, manifest);
		return { workspace, name: expectedName, version: manifest.version };
	});
	for (const pkg of inventory.slice(0, -1)) {
		if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)-scramjet\.(?:0|[1-9]\d*)$/.test(pkg.version)) {
			fail(`${pkg.name} must use an X.Y.Z-scramjet.N runtime version`);
		}
	}
	const scramjet = inventory.at(-1);
	if (!/^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/.test(scramjet.version)) {
		fail("@leanandmean/scramjet must use a stable X.Y.Z version");
	}
	const versions = new Map(inventory.map(({ name, version }) => [name, version]));
	for (const { name } of inventory) validateInternalDependencies(manifests.get(name), name, versions, `${name} manifest`);

	const lock = requireObject(parseJson(readMetadata("package-lock.json"), "package-lock.json"), "package-lock.json");
	if (lock.lockfileVersion !== 3) fail("package-lock.json must use lockfileVersion 3");
	const packages = requireObject(lock.packages, "package-lock.json packages");
	const workspaceKeys = Object.keys(packages).filter((path) => /^packages\/[^/]+$/.test(path)).sort();
	const expectedWorkspaceKeys = inventory.map(({ workspace }) => workspace).sort();
	if (JSON.stringify(workspaceKeys) !== JSON.stringify(expectedWorkspaceKeys)) {
		fail("package-lock.json must contain exactly the five release workspaces");
	}
	for (const pkg of inventory) {
		const workspaceRecord = requireObject(packages[pkg.workspace], `package-lock.json ${pkg.workspace}`);
		if (workspaceRecord.name !== pkg.name || workspaceRecord.version !== pkg.version) {
			fail(`package-lock.json ${pkg.workspace} must match ${pkg.name}@${pkg.version}`);
		}
		validateInternalDependencies(workspaceRecord, pkg.name, versions, `package-lock.json ${pkg.workspace}`);
	}
	const linkKeys = Object.keys(packages).filter((path) => /^node_modules\/@leanandmean\/[^/]+$/.test(path)).sort();
	const expectedLinkKeys = inventory.map(({ name }) => `node_modules/${name}`).sort();
	if (JSON.stringify(linkKeys) !== JSON.stringify(expectedLinkKeys)) {
		fail("package-lock.json must contain exactly the five release workspace links");
	}
	for (const pkg of inventory) {
		const link = requireObject(packages[`node_modules/${pkg.name}`], `package-lock.json link for ${pkg.name}`);
		if (link.link !== true || link.resolved !== pkg.workspace) {
			fail(`package-lock.json link for ${pkg.name} must resolve to ${pkg.workspace}`);
		}
	}
	return inventory;
}

export function loadInventory(root = REPO_ROOT) {
	return parseInventory((path) => readFileSync(join(root, path), "utf8"));
}

export function loadPreflightInventory(confirmedSha, git = (args) => run("git", args)) {
	if (!/^[0-9a-f]{40}$/.test(confirmedSha ?? "")) fail("confirmed SHA must be a canonical 40-character commit SHA");
	git(["cat-file", "-e", `${confirmedSha}^{commit}`]);
	if (git(["rev-parse", "HEAD"]) !== confirmedSha) fail("confirmed SHA must equal checked-out HEAD");
	if (git(["status", "--porcelain=v1", "--", ...RELEASE_METADATA_PATHS]) !== "") {
		fail("release manifests and package-lock.json must match checked-out HEAD");
	}
	return parseInventory((path) => git(["show", `${confirmedSha}:${path}`]));
}

export function validateIdentity(inventory, env = process.env, git = (args) => run("git", args)) {
	if (env.GITHUB_EVENT_NAME !== "push") fail("GITHUB_EVENT_NAME must be push");
	if (env.GITHUB_RUN_ATTEMPT !== "1") fail("GITHUB_RUN_ATTEMPT must be 1");
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
	return parseJson(run("npm", [...args, "--registry", REGISTRY_URL]), description);
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
	const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-scramjet\.(0|[1-9]\d*))?$/.exec(version);
	if (!match) fail(`unsupported version format: ${version}`);
	return {
		core: match.slice(1, 4).map(BigInt),
		prerelease: match[4] === undefined ? null : BigInt(match[4]),
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
		if (versions.includes(pkg.version)) fail(`${pkg.name}@${pkg.version} already exists; every release requires five fresh versions`);
		if (typeof distTags.latest !== "string") fail(`${pkg.name} has no string-valued latest dist-tag`);
		if (compareVersions(pkg.version, distTags.latest) <= 0) {
			fail(`${pkg.name}@${pkg.version} is not newer than latest ${distTags.latest}`);
		}
		return { ...pkg, distTags };
	});
	for (const pkg of plan) {
		console.log(`${pkg.name}@${pkg.version}: missing; dist-tags=${JSON.stringify(pkg.distTags)}`);
	}
	return plan;
}

function tagsEqual(left, right) {
	const leftEntries = Object.entries(left).sort(([a], [b]) => a.localeCompare(b));
	const rightEntries = Object.entries(right).sort(([a], [b]) => a.localeCompare(b));
	return JSON.stringify(leftEntries) === JSON.stringify(rightEntries);
}

export async function pollRead(description, operation, dependencies = {}) {
	const attempts = dependencies.attempts ?? POST_PUBLISH_ATTEMPTS;
	const delayMs = dependencies.delayMs ?? POST_PUBLISH_DELAY_MS;
	const sleep = dependencies.sleep ?? ((duration) => new Promise((resolveSleep) => setTimeout(resolveSleep, duration)));
	const retryIf = dependencies.retryIf ?? (() => true);
	let lastError;
	for (let attempt = 1; attempt <= attempts; attempt += 1) {
		try {
			return await operation();
		} catch (error) {
			if (!retryIf(error)) throw error;
			lastError = error;
			if (attempt === attempts) break;
			console.log(`${description} not ready (attempt ${attempt}/${attempts}); retrying in ${delayMs}ms`);
			await sleep(delayMs);
		}
	}
	fail(`${description} did not converge after ${attempts} attempts: ${lastError?.message ?? String(lastError)}`);
}

function npmErrorCode(error) {
	for (const output of [error?.stdout, error?.stderr]) {
		const text = Buffer.isBuffer(output) ? output.toString("utf8") : output;
		if (typeof text !== "string") continue;
		try {
			const parsed = JSON.parse(text);
			if (parsed !== null && !Array.isArray(parsed) && typeof parsed === "object" && typeof parsed.error?.code === "string") {
				return parsed.error.code;
			}
		} catch {}
	}
	return undefined;
}

function isTransientTransportError(error) {
	const npmCode = npmErrorCode(error);
	return (
		["AbortError", "TimeoutError"].includes(error?.name) ||
		["ETIMEDOUT", "ECONNRESET", "ECONNREFUSED", "EAI_AGAIN", "ENETUNREACH"].includes(error?.code) ||
		["E404", "E408", "E429"].includes(npmCode) ||
		/^E5\d\d$/.test(npmCode ?? "") ||
		error?.status === 404 ||
		error?.status === 408 ||
		error?.status === 429 ||
		(typeof error?.status === "number" && error.status >= 500 && error.status <= 599) ||
		/fetch failed/.test(error?.message ?? "")
	);
}

function registryPropagationError(message) {
	const error = new Error(message);
	error.code = "REGISTRY_PROPAGATION";
	return error;
}

function isRegistryVisibilityRetryError(error) {
	return error?.code === "REGISTRY_PROPAGATION" || isTransientTransportError(error);
}

export function isTransientReadError(error) {
	return isRegistryVisibilityRetryError(error);
}

export function publishPackage(pkg, command = run, timeoutMs = PUBLISH_TIMEOUT_MS) {
	try {
		command(
			"npm",
			[
				"publish",
				"-w",
				pkg.workspace,
				"--access",
				"public",
				"--provenance",
				"--tag",
				"latest",
				"--registry",
				REGISTRY_URL,
			],
			{ stdio: "inherit", timeout: timeoutMs },
		);
	} catch (error) {
		const output = [error?.stderr, error?.stdout]
			.map((value) => (Buffer.isBuffer(value) ? value.toString("utf8") : value))
			.filter((value) => typeof value === "string" && value.trim().length > 0)
			.map((value) => value.trim())
			.join("\n");
		const detail = output || error?.message;
		throw new Error(
			`npm publish for ${pkg.name}@${pkg.version} failed after publication began; publication state is ambiguous. Do not retry publication; inspect registry state read-only and prepare another five-fresh forward release.${detail ? ` Cause: ${detail}` : ""}`,
			{ cause: error },
		);
	}
}

export async function publish(inventory, dependencies = {}) {
	const plan = preflight(inventory);
	for (const pkg of plan) {
		const currentVersions = requireVersions(
			npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions before publish`),
			pkg.name,
		);
		if (currentVersions.includes(pkg.version)) fail(`${pkg.name}@${pkg.version} appeared after preflight`);
		const currentTags = requireDistTags(
			npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags before publish`),
			pkg.name,
		);
		if (!tagsEqual(currentTags, pkg.distTags)) fail(`${pkg.name} dist-tags changed after preflight`);
		if (typeof currentTags.latest !== "string" || compareVersions(pkg.version, currentTags.latest) <= 0) {
			fail(`${pkg.name}@${pkg.version} is not newer than latest ${currentTags.latest}`);
		}
		publishPackage(pkg);
		try {
			const publishedTags = await pollRead(
				`${pkg.name}@${pkg.version} registry visibility`,
				async () => {
					const publishedVersions = requireVersions(
						npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions after publish`),
						pkg.name,
					);
					if (!publishedVersions.includes(pkg.version)) {
						throw registryPropagationError(`${pkg.name}@${pkg.version} was not visible after publish`);
					}
					const tags = requireDistTags(
						npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags after publish`),
						pkg.name,
					);
					if (tags.latest !== pkg.version) {
						throw registryPropagationError(`${pkg.name} latest did not move to ${pkg.version}`);
					}
					return tags;
				},
				{ ...dependencies.pollDependencies, retryIf: isRegistryVisibilityRetryError },
			);
			const beforeNonLatest = { ...pkg.distTags };
			const afterNonLatest = { ...publishedTags };
			delete beforeNonLatest.latest;
			delete afterNonLatest.latest;
			if (!tagsEqual(beforeNonLatest, afterNonLatest)) fail(`${pkg.name} non-latest dist-tags changed during publish`);
			await pollRead(
				`${pkg.name}@${pkg.version} attestation metadata`,
				async () => {
					const output = run("npm", [
						"view",
						`${pkg.name}@${pkg.version}`,
						"dist.attestations.url",
						"--json",
						"--registry",
						REGISTRY_URL,
					]);
					if (output === "") throw registryPropagationError(`${pkg.name}@${pkg.version} has no attestation URL`);
					const url = parseJson(output, `${pkg.name}@${pkg.version} attestation URL`);
					if (typeof url !== "string") fail(`${pkg.name}@${pkg.version} attestation URL must be a string`);
				},
				{ ...dependencies.pollDependencies, retryIf: isTransientReadError },
			);
		} catch (error) {
			throw new Error(
				`Post-publish verification for ${pkg.name}@${pkg.version} failed; publication state is ambiguous. Do not retry publication; inspect registry state read-only and prepare another five-fresh forward release. Cause: ${error?.message ?? String(error)}`,
				{ cause: error },
			);
		}
	}
}

async function main() {
	const [mode, ...args] = process.argv.slice(2);
	if (!["validate", "preflight", "publish"].includes(mode)) {
		fail("usage: release.mjs <validate|publish> | release.mjs preflight <confirmed-sha>");
	}
	if (mode === "preflight") {
		if (args.length !== 1) fail("usage: release.mjs preflight <confirmed-sha>");
		preflight(loadPreflightInventory(args[0]));
		return;
	}
	if (args.length !== 0) fail(`usage: release.mjs ${mode}`);
	const inventory = loadInventory();
	validateIdentity(inventory);
	if (mode === "validate") return;
	if (mode === "publish") await publish(inventory);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	main().catch((error) => {
		console.error(`release: ${error.message}`);
		process.exitCode = 1;
	});
}
