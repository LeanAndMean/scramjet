#!/usr/bin/env node

import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPOSITORY_URL = "git+https://github.com/LeanAndMean/scramjet.git";
const REGISTRY_URL = "https://registry.npmjs.org/";
const WORKFLOW_PATH = ".github/workflows/release.yml";
export const READ_TIMEOUT_MS = 60_000;
export const PUBLISH_TIMEOUT_MS = 10 * 60_000;
export const POST_PUBLISH_BUDGET_MS = 10 * 60_000;
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

function parseManifests(readMetadata) {
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
	return inventory;
}

function parseInventory(readMetadata) {
	const inventory = parseManifests(readMetadata);
	const versions = new Map(inventory.map(({ name, version }) => [name, version]));
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
	if (!/^[1-9]\d*$/.test(env.GITHUB_RUN_ID ?? "")) fail("GITHUB_RUN_ID must be a canonical positive decimal ID");
	if (!/^[1-9]\d*$/.test(env.GITHUB_RUN_ATTEMPT ?? "")) fail("GITHUB_RUN_ATTEMPT must be a canonical positive decimal attempt");
	const scramjet = inventory.find(({ name }) => name === "@leanandmean/scramjet");
	const expectedRef = `refs/tags/v${scramjet.version}`;
	if (env.GITHUB_REF !== expectedRef) fail(`GITHUB_REF must be ${expectedRef}`);
	if (!/^[0-9a-f]{40}$/.test(env.GITHUB_SHA ?? "")) fail("GITHUB_SHA must be a 40-character commit SHA");
	const expectedWorkflowRef = `LeanAndMean/scramjet/${WORKFLOW_PATH}@${expectedRef}`;
	if (env.GITHUB_WORKFLOW_REF !== expectedWorkflowRef) fail(`GITHUB_WORKFLOW_REF must be ${expectedWorkflowRef}`);
	const head = git(["rev-parse", "HEAD"]);
	if (head !== env.GITHUB_SHA) fail("checked-out HEAD must equal GITHUB_SHA");
	return { ref: expectedRef, sha: env.GITHUB_SHA, runId: env.GITHUB_RUN_ID, attempt: env.GITHUB_RUN_ATTEMPT };
}

function npmJson(args, description, timeout = READ_TIMEOUT_MS) {
	return parseJson(run("npm", [...args, "--registry", REGISTRY_URL], { timeout }), description);
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

function readAttestations(pkg, timeout = READ_TIMEOUT_MS) {
	const description = `${pkg.name}@${pkg.version} attestations`;
	const output = run(
		"npm",
		["view", `${pkg.name}@${pkg.version}`, "dist.attestations", "--json", "--registry", REGISTRY_URL],
		{ timeout },
	);
	if (output === "") throw registryPropagationError(`${pkg.name}@${pkg.version} has no attestation URL`);
	return parseJson(output, description);
}

function requireAttestations(value, pkg) {
	const attestations = requireObject(value, `${pkg.name}@${pkg.version} attestations`);
	if (typeof attestations.url !== "string" || attestations.url.length === 0) {
		throw registryPropagationError(`${pkg.name}@${pkg.version} has no attestation URL`);
	}
	const provenance = requireObject(attestations.provenance, `${pkg.name}@${pkg.version} provenance`);
	if (provenance.predicateType !== "https://slsa.dev/provenance/v1") {
		throw registryPropagationError(`${pkg.name}@${pkg.version} has no SLSA v1 provenance predicate`);
	}
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

function sha512Digest(integrity, description) {
	if (typeof integrity !== "string" || !/^sha512-[A-Za-z0-9+/]{86}==$/.test(integrity)) {
		fail(`${description} must be a SHA-512 integrity string`);
	}
	const digest = Buffer.from(integrity.slice(7), "base64");
	if (digest.length !== 64 || digest.toString("base64") !== integrity.slice(7)) {
		fail(`${description} must contain a canonical 64-byte SHA-512 digest`);
	}
	return digest;
}

export function packCandidates(inventory, directory) {
	if (inventory.length !== INVENTORY.length) fail("candidate inventory must contain all five release workspaces");
	const destination = resolve(directory);
	const manifests = new Map();
	const filenames = new Set();
	const candidates = inventory.map((pkg, index) => {
		const [workspace, name] = INVENTORY[index];
		if (pkg.workspace !== workspace || pkg.name !== name) fail(`candidate inventory must follow release order at ${workspace}`);
		parseVersion(pkg.version);
		const description = `${pkg.name}@${pkg.version}`;
		const results = parseJson(
			run("npm", ["pack", "--json", "-w", pkg.workspace, "--pack-destination", destination]),
			`${description} npm pack output`,
		);
		if (!Array.isArray(results) || results.length !== 1 || results[0]?.name !== pkg.name || results[0]?.version !== pkg.version) {
			fail(`${description} npm pack must report exactly the intended package`);
		}
		const expected = `${pkg.name.slice(1).replace("/", "-")}-${pkg.version}.tgz`;
		if (results[0].filename !== expected || filenames.has(expected)) {
			fail(`${description} npm pack must report a unique expected filename in the candidate directory`);
		}
		filenames.add(expected);
		const tarballPath = join(destination, expected);
		const stat = lstatSync(tarballPath);
		if (!stat.isFile()) fail(`${description} candidate archive must be a regular non-symlink file: ${tarballPath}`);
		const manifestPath = `${pkg.workspace}/package.json`;
		manifests.set(manifestPath, run("tar", ["-xOzf", tarballPath, "package/package.json"]));
		const integrity = `sha512-${createHash("sha512").update(readFileSync(tarballPath)).digest("base64")}`;
		sha512Digest(results[0].integrity, `${description} npm pack integrity`);
		if (results[0].integrity !== integrity) fail(`${description} npm pack integrity differs from candidate archive bytes`);
		return { ...pkg, tarballPath, integrity };
	});
	const packed = parseManifests((path) => manifests.get(path));
	for (let index = 0; index < inventory.length; index += 1) {
		if (packed[index].version !== inventory[index].version) {
			fail(`${inventory[index].name} packed version differs from committed inventory`);
		}
	}
	return candidates;
}

export async function reconcileCandidates(candidates, dependencies = {}) {
	const observedPresence = dependencies.observedPresence ?? new Set();
	const results = [];
	for (const pkg of candidates) {
		const description = `${pkg.name}@${pkg.version}`;
		const result = await pollRead(
			`${description} candidate reconciliation`,
			async ({ remainingMs }) => {
				const readTimeout = () => {
					const timeout = Math.floor(Math.min(READ_TIMEOUT_MS, remainingMs()));
					if (timeout <= 0) throw registryPropagationError(`${description} observation budget expired`);
					return timeout;
				};
				const versions = requireVersions(npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions`, readTimeout()), pkg.name);
				if (versions.includes(pkg.version)) observedPresence.add(pkg.name);
				const distTags = requireDistTags(npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags`, readTimeout()), pkg.name);
				if (typeof distTags.latest !== "string") throw registryPropagationError(`${pkg.name} has no latest dist-tag yet`);
				const order = compareVersions(pkg.version, distTags.latest);
				if (order < 0) fail(`${description} is superseded by latest ${distTags.latest}`);
				if (order === 0) observedPresence.add(pkg.name);
				if (observedPresence.has(pkg.name)) {
					if (!versions.includes(pkg.version) && order === 0) {
						throw registryPropagationError(`${pkg.name} latest is ${pkg.version} but versions omit it`);
					}
					const output = run("npm", ["view", description, "dist", "--json", "--registry", REGISTRY_URL], { timeout: readTimeout() });
					if (output === "") throw registryPropagationError(`${description} has no dist metadata yet`);
					const dist = requireObject(parseJson(output, `${description} dist`), `${description} dist`);
					if (dist.integrity === undefined) throw registryPropagationError(`${description} has no integrity yet`);
					sha512Digest(dist.integrity, `${description} registry integrity`);
					if (dist.integrity !== pkg.integrity) fail(`${description} registry integrity differs from candidate archive`);
					if (dist.attestations === undefined) throw registryPropagationError(`${description} has no attestations yet`);
					requireAttestations(dist.attestations, pkg);
					if (order !== 0) throw registryPropagationError(`${pkg.name} latest is not ${pkg.version} yet`);
					return { ...pkg, status: "retained", distTags: { ...distTags } };
				}
				if (dependencies.requirePresence) throw registryPropagationError(`${description} was not visible after publish`);
				return { ...pkg, status: "missing", distTags: { ...distTags } };
			},
			{ ...dependencies, retryIf: isRegistryVisibilityRetryError },
		);
		results.push(result);
	}
	return results;
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
	const budgetMs = dependencies.budgetMs ?? POST_PUBLISH_BUDGET_MS;
	const delayMs = dependencies.delayMs ?? POST_PUBLISH_DELAY_MS;
	const now = dependencies.now ?? (() => performance.now());
	const sleep = dependencies.sleep ?? ((duration) => new Promise((resolveSleep) => setTimeout(resolveSleep, duration)));
	const retryIf = dependencies.retryIf ?? (() => true);
	const startedAt = now();
	let observations = 0;
	let lastError;
	while (true) {
		const elapsedMs = now() - startedAt;
		const remainingMs = budgetMs - elapsedMs;
		if (observations > 0 && remainingMs <= 0) break;
		observations += 1;
		try {
			const result = await operation({ remainingMs: () => Math.max(0, budgetMs - (now() - startedAt)) });
			if (now() - startedAt < budgetMs) return result;
			lastError = new Error("observation completed after the budget expired");
			break;
		} catch (error) {
			if (!retryIf(error)) throw error;
			lastError = error;
		}
		const afterOperationMs = now() - startedAt;
		const remainingAfterOperationMs = budgetMs - afterOperationMs;
		if (remainingAfterOperationMs <= 0) break;
		const sleepMs = Math.min(delayMs, remainingAfterOperationMs);
		console.log(`${description} not ready (observation ${observations}); retrying in ${sleepMs}ms`);
		await sleep(sleepMs);
	}
	const elapsedMs = Math.max(0, now() - startedAt);
	throw new Error(
		`${description} did not converge within ${budgetMs}ms after ${elapsedMs}ms and ${observations} observations: ${lastError?.message ?? String(lastError)}`,
		{ cause: lastError },
	);
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

export function publishPackage(pkg, command = spawnSync, timeoutMs = PUBLISH_TIMEOUT_MS) {
	if (typeof pkg.tarballPath !== "string" || !pkg.tarballPath.startsWith("/")) {
		fail(`${pkg.name}@${pkg.version} requires a checked absolute candidate archive`);
	}
	let result;
	try {
		result = command(
			"npm",
			["publish", pkg.tarballPath, "--access", "public", "--provenance", "--tag", "latest", "--registry", REGISTRY_URL],
			{ cwd: REPO_ROOT, encoding: "utf8", timeout: timeoutMs },
		);
	} catch (error) {
		throw new Error(`npm publish for ${pkg.name}@${pkg.version} failed: ${error?.message ?? String(error)}`, { cause: error });
	}
	if (result.stdout) process.stdout.write(result.stdout);
	if (result.stderr) process.stderr.write(result.stderr);
	if (result.error || result.status !== 0 || result.signal) {
		const detail = [result.error?.message, result.stderr, result.stdout, `status=${result.status}; signal=${result.signal}`]
			.filter(Boolean).join("; ");
		throw new Error(`npm publish for ${pkg.name}@${pkg.version} failed: ${detail}`, { cause: result });
	}
}

function checkCandidateBytes(pkg) {
	const stat = lstatSync(pkg.tarballPath);
	if (!stat.isFile()) fail(`${pkg.name}@${pkg.version} candidate archive is no longer a regular file`);
	const integrity = `sha512-${createHash("sha512").update(readFileSync(pkg.tarballPath)).digest("base64")}`;
	if (integrity !== pkg.integrity) fail(`${pkg.name}@${pkg.version} candidate archive changed before publication`);
}

export async function publish(inventory, dependencies = {}) {
	const directory = mkdtempSync(join(tmpdir(), "scramjet-release-candidates-"));
	const now = dependencies.pollDependencies?.now ?? (() => performance.now());
	const startedAt = now();
	const states = inventory.map(() => "unattempted");
	const observedPresence = new Set();
	let phase = "candidate preparation";
	const elapsed = () => Math.max(0, Math.round(now() - startedAt));
	try {
		const candidates = packCandidates(inventory, directory);
		phase = "complete-set reconciliation";
		const plan = await reconcileCandidates(candidates, { ...dependencies.pollDependencies, observedPresence });
		for (const [index, pkg] of plan.entries()) {
			phase = `immediate reread for ${pkg.name}@${pkg.version}`;
			const [current] = await reconcileCandidates([pkg], { ...dependencies.pollDependencies, observedPresence });
			if (current.status === "retained") {
				if (pkg.status === "missing") {
					const before = { ...pkg.distTags };
					const after = { ...current.distTags };
					delete before.latest;
					delete after.latest;
					if (!tagsEqual(before, after)) fail(`${pkg.name} non-latest dist-tags changed before publication`);
				} else if (!tagsEqual(current.distTags, pkg.distTags)) {
					fail(`${pkg.name} dist-tags changed after reconciliation`);
				}
				states[index] = "matching content retained; skipped (original publisher not established)";
				console.log(`${pkg.name}@${pkg.version}: ${states[index]}`);
				continue;
			}
			if (pkg.status === "retained") fail(`${pkg.name}@${pkg.version} previously observed present but is now missing`);
			if (!tagsEqual(current.distTags, pkg.distTags)) fail(`${pkg.name} dist-tags changed after reconciliation`);
			if (compareVersions(pkg.version, current.distTags.latest) <= 0) fail(`${pkg.name}@${pkg.version} is not newer than latest`);
			checkCandidateBytes(pkg);
			phase = `publish command for ${pkg.name}@${pkg.version}`;
			let commandError;
			try {
				publishPackage(pkg, dependencies.publishCommand);
			} catch (error) {
				commandError = error;
			}
			states[index] = commandError ? "acceptance ambiguous" : "command accepted; observation incomplete";
			phase = `post-publish observation for ${pkg.name}@${pkg.version}`;
			try {
				const [observed] = await reconcileCandidates([pkg], { ...dependencies.pollDependencies, observedPresence, requirePresence: true });
				if (observed.status !== "retained") throw registryPropagationError(`${pkg.name}@${pkg.version} was not visible after publish`);
				const before = { ...pkg.distTags };
				const after = { ...observed.distTags };
				delete before.latest;
				delete after.latest;
				if (!tagsEqual(before, after)) fail(`${pkg.name} non-latest dist-tags changed during publish`);
			} catch (observationError) {
				throw new Error(
					`${pkg.name}@${pkg.version} observation failed: ${observationError.message}${commandError ? `; publish command: ${commandError.message}` : ""}`,
					{ cause: new AggregateError([...(commandError ? [commandError] : []), observationError]) },
				);
			}
			states[index] = commandError ? "command errored; matching content observed (publisher not established)" : "command accepted; matching content observed";
			console.log(`${pkg.name}@${pkg.version}: ${states[index]} after ${elapsed()}ms`);
		}
		console.log(`publication summary: ${states.map((status, index) => `${inventory[index].name}@${inventory[index].version}: ${status}`).join("; ")}; final verification: pending`);
	} catch (error) {
		const summary = states.map((status, index) => `${inventory[index].name}@${inventory[index].version}: ${status}`).join("; ");
		throw new Error(
			`${error.message}\nPublication summary: ${summary}; failed phase: ${phase}; elapsed ${elapsed()}ms; budget ${POST_PUBLISH_BUDGET_MS}ms; final verification: not completed. Inspect the cause and prior attempt facts before authorizing a same-run rerun. A differing candidate cannot be retained or overwritten: diagnose build drift; if intended content differs, use a new five-fresh forward release. Superseded latest or altered tags must not be repaired. If publication cannot be authorized, use forward recovery.`,
			{ cause: error },
		);
	} finally {
		rmSync(directory, { recursive: true, force: true });
	}
}

function isRetryableInstallError(error, inventory) {
	const outputs = [error?.stdout, error?.stderr];
	let npmError;
	for (const output of outputs) {
		const text = Buffer.isBuffer(output) ? output.toString("utf8") : output;
		if (typeof text !== "string") continue;
		try {
			const parsed = JSON.parse(text);
			if (parsed?.error && typeof parsed.error.code === "string") {
				npmError = parsed.error;
				break;
			}
		} catch {}
	}
	if (!npmError) return false;
	if (npmError.code === "E404") {
		if (typeof npmError.summary !== "string") return false;
		const match = /^404 Not Found - GET (https:\/\/[^\s]+?)(?: - |$)/.exec(npmError.summary);
		if (!match) return false;
		return inventory.some(({ name, version }) => {
			const basename = name.slice("@leanandmean/".length);
			return match[1] === `https://registry.npmjs.org/@leanandmean/${basename}/-/${basename}-${version}.tgz`;
		});
	}
	return ["E408", "E429", "ETIMEDOUT", "ECONNRESET", "ECONNREFUSED", "EAI_AGAIN", "ENETUNREACH"].includes(npmError.code) || /^E5\d\d$/.test(npmError.code);
}

function commandFailureDetail(error) {
	for (const value of [error?.stderr, error?.stdout]) {
		const text = Buffer.isBuffer(value) ? value.toString("utf8") : value;
		if (typeof text === "string" && text.trim().length > 0) return text.trim();
	}
	return error?.message ?? String(error);
}

export async function verify(inventory, dependencies = {}) {
	let root;
	let phase = "metadata";
	const observed = [];
	const refs = (packages) => packages.map(({ name, version }) => `${name}@${version}`).join(", ") || "none";
	const readVerificationMetadata = dependencies.readVerificationMetadata ?? ((pkg, { remainingMs }) => {
		const readTimeout = () => {
			const timeout = Math.floor(Math.min(READ_TIMEOUT_MS, remainingMs()));
			if (timeout <= 0) throw registryPropagationError(`${pkg.name}@${pkg.version} observation budget expired`);
			return timeout;
		};
		const versions = requireVersions(
			npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions during verification`, readTimeout()),
			pkg.name,
		);
		if (!versions.includes(pkg.version)) {
			throw registryPropagationError(`${pkg.name}@${pkg.version} is not visible`);
		}
		const distTags = requireDistTags(
			npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags during verification`, readTimeout()),
			pkg.name,
		);
		if (distTags.latest !== pkg.version) {
			throw registryPropagationError(`${pkg.name} latest is not ${pkg.version}`);
		}
		requireAttestations(readAttestations(pkg, readTimeout()), pkg);
	});
	try {
		for (const pkg of inventory) {
			phase = `metadata for ${pkg.name}@${pkg.version}`;
			await pollRead(
				`${pkg.name}@${pkg.version} verification metadata`,
				(context) => readVerificationMetadata(pkg, context),
				{ ...dependencies.pollDependencies, retryIf: isRegistryVisibilityRetryError },
			);
			observed.push(pkg);
		}

		phase = "install";
		const scramjet = inventory.at(-1);
		await pollRead(
			"exact release install",
			({ remainingMs }) => {
				const attemptRoot = mkdtempSync(join(tmpdir(), "scramjet-release-verification-"));
				try {
					const project = join(attemptRoot, "project");
					const cache = join(attemptRoot, "cache");
					const home = join(attemptRoot, "home");
					const xdg = join(attemptRoot, "xdg");
					for (const path of [project, cache, home, xdg]) mkdirSync(path);
					writeFileSync(join(project, "package.json"), JSON.stringify({ private: true }));
					const env = { ...process.env, HOME: home, XDG_DATA_HOME: xdg, npm_config_cache: cache };
					const timeout = Math.floor(Math.min(PUBLISH_TIMEOUT_MS, remainingMs()));
					if (timeout <= 0) throw new Error("install observation budget expired");
					run(
						"npm",
						["install", "--save-exact", `${scramjet.name}@${scramjet.version}`, "--ignore-scripts=false", "--json", "--registry", REGISTRY_URL],
						{ cwd: project, env, timeout },
					);
					if (remainingMs() <= 0) throw new Error("install completed after the budget expired");
					root = attemptRoot;
				} finally {
					if (root !== attemptRoot) rmSync(attemptRoot, { recursive: true, force: true });
				}
			},
			{ ...dependencies.pollDependencies, retryIf: (error) => isRetryableInstallError(error, inventory) },
		);
		const project = join(root, "project");
		const env = { ...process.env, HOME: join(root, "home"), XDG_DATA_HOME: join(root, "xdg"), npm_config_cache: join(root, "cache") };
		phase = "installed closure";
		for (const pkg of inventory) {
			const manifestPath = join(project, "node_modules", pkg.name, "package.json");
			const manifest = requireObject(parseJson(readFileSync(manifestPath, "utf8"), manifestPath), manifestPath);
			if (manifest.name !== pkg.name || manifest.version !== pkg.version) {
				fail(`${pkg.name} installed version must be exactly ${pkg.version}`);
			}
		}
		phase = "signature audit";
		run("npm", ["audit", "signatures", "--registry", REGISTRY_URL], {
			cwd: project,
			env,
			timeout: PUBLISH_TIMEOUT_MS,
		});
		phase = "installed runtime smoke";
		const packageRoot = join(project, "node_modules", "@leanandmean", "scramjet");
		const smokeRoot = join(root, "installed-runtime-smoke");
		const runInstalledRuntimeSmoke = dependencies.runInstalledRuntimeSmoke ?? ((installedRoot, workDir) => {
			run(process.execPath, [join(REPO_ROOT, ".github", "scripts", "installed-runtime-smoke.mjs"), installedRoot, workDir], {
				env,
				timeout: PUBLISH_TIMEOUT_MS,
			});
		});
		runInstalledRuntimeSmoke(packageRoot, smokeRoot);
		phase = "installed CLI probe";
		try {
			run(join(project, "node_modules", ".bin", "scramjet"), ["--help"], { cwd: project, env });
		} catch (error) {
			throw new Error(`installed scramjet --help failed: ${commandFailureDetail(error)}`, { cause: error });
		}
		console.log("final verification: completed");
	} catch (error) {
		throw new Error(
			`Published release verification failed. Verification metadata observed: ${refs(observed)}. Final verification is independently rerunnable after inspecting the immutable tag/SHA, prior attempt facts and failure; it does not alter publication acceptance. Diagnose deterministic defects in immutable artifacts before a five-fresh forward release. failed phase: ${phase}; final verification: not completed. Cause: ${commandFailureDetail(error)}`,
			{ cause: error },
		);
	} finally {
		if (root !== undefined) rmSync(root, { recursive: true, force: true });
	}
}

async function main() {
	const [mode, ...args] = process.argv.slice(2);
	if (!["validate", "preflight", "publish", "verify"].includes(mode)) {
		fail("usage: release.mjs <validate|publish|verify> | release.mjs preflight <confirmed-sha>");
	}
	if (mode === "preflight") {
		if (args.length !== 1) fail("usage: release.mjs preflight <confirmed-sha>");
		preflight(loadPreflightInventory(args[0]));
		return;
	}
	if (args.length !== 0) fail(`usage: release.mjs ${mode}`);
	const inventory = loadInventory();
	const identity = validateIdentity(inventory);
	console.log(`release run ${identity.runId} attempt ${identity.attempt}`);
	if (mode === "validate") return;
	if (mode === "publish") {
		const committedInventory = loadPreflightInventory(identity.sha);
		if (identity.attempt === "1") preflight(committedInventory);
		await publish(committedInventory);
	}
	if (mode === "verify") await verify(inventory);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	main().catch((error) => {
		console.error(`release: ${error.message}`);
		process.exitCode = 1;
	});
}
