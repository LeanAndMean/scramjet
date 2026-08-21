#!/usr/bin/env node

import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPOSITORY_URL = "git+https://github.com/LeanAndMean/scramjet.git";
const REGISTRY_URL = "https://registry.npmjs.org/";
const WORKFLOW_REPOSITORY = "https://github.com/LeanAndMean/scramjet";
const WORKFLOW_PATH = ".github/workflows/release.yml";
const SLSA_PREDICATE = "https://slsa.dev/provenance/v1";
const GITHUB_OIDC_ISSUER = "https://token.actions.githubusercontent.com";
const CERTIFICATE_IDENTITY =
	"^https://github\\.com/LeanAndMean/scramjet/\\.github/workflows/release\\.yml@refs/tags/v\\d+\\.\\d+\\.\\d+$";
export const READ_TIMEOUT_MS = 60_000;
export const PUBLISH_TIMEOUT_MS = 10 * 60_000;
export const POST_PUBLISH_ATTEMPTS = 6;
export const POST_PUBLISH_DELAY_MS = 10_000;
const INVENTORY = [
	["packages/tui", "@leanandmean/tui"],
	["packages/ai", "@leanandmean/ai"],
	["packages/agent", "@leanandmean/agent"],
	["packages/coding-agent", "@leanandmean/coding-agent"],
	["packages/scramjet", "@leanandmean/scramjet"],
];
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const PREFLIGHT_READS_PER_PACKAGE = 2;
const PRESENT_PACKAGE_READS = 3;
const PRE_PUBLISH_READS = 2;
const POST_PUBLISH_READS_PER_ATTEMPT = 4;
const SIGSTORE_TRUST_READ_WINDOWS = 10;
const MAX_READ_WINDOWS =
	INVENTORY.length *
		(PREFLIGHT_READS_PER_PACKAGE +
			PRESENT_PACKAGE_READS +
			PRE_PUBLISH_READS +
			POST_PUBLISH_ATTEMPTS * POST_PUBLISH_READS_PER_ATTEMPT) +
	SIGSTORE_TRUST_READ_WINDOWS;
export const RELEASE_HELPER_MAX_MINUTES = Math.ceil(
	(MAX_READ_WINDOWS * READ_TIMEOUT_MS +
		INVENTORY.length * PUBLISH_TIMEOUT_MS +
		INVENTORY.length * 2 * (POST_PUBLISH_ATTEMPTS - 1) * POST_PUBLISH_DELAY_MS) /
		60_000,
);
let defaultProvenanceVerifier;

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
		if (
			manifest.publishConfig?.access !== "public" ||
			Object.keys(manifest.publishConfig).length !== 1
		) {
			fail(`${expectedName} publishConfig must contain only public access`);
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
		if (typeof distTags.latest !== "string") fail(`${pkg.name} has no string-valued latest dist-tag`);
		if (present && distTags.latest !== pkg.version) {
			fail(`${pkg.name}@${pkg.version} is present but is not current latest ${distTags.latest}`);
		}
		if (!present && compareVersions(pkg.version, distTags.latest) <= 0) {
			fail(`${pkg.name}@${pkg.version} is not newer than latest ${distTags.latest}`);
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
	const digest = Buffer.from(match[1], "base64");
	if (digest.length !== 64 || digest.toString("base64") !== match[1]) {
		fail("registry integrity must contain one canonical 64-byte SHA-512 digest");
	}
	return digest.toString("hex");
}

export async function fetchAttestationJson(url, fetchImpl = fetch, timeoutMs = READ_TIMEOUT_MS) {
	const parsed = new URL(url);
	if (parsed.protocol !== "https:" || parsed.hostname !== "registry.npmjs.org") {
		fail("attestation URL must use the npm registry over HTTPS");
	}
	const response = await fetchImpl(parsed, { signal: AbortSignal.timeout(timeoutMs) });
	if (!response.ok) fail(`attestation request failed with HTTP ${response.status}`);
	return response.json();
}

export async function createProvenanceVerifier(createVerifier) {
	let verifier;
	try {
		createVerifier ??= (await import("sigstore")).createVerifier;
		verifier = await createVerifier({
			certificateIssuer: GITHUB_OIDC_ISSUER,
			certificateIdentityURI: CERTIFICATE_IDENTITY,
			ctLogThreshold: 1,
			tlogThreshold: 1,
			timeout: READ_TIMEOUT_MS,
			retry: 0,
		});
	} catch (error) {
		fail(`Sigstore trust initialization failed: ${error?.message ?? String(error)}`);
	}
	return (bundle) => verifier.verify(bundle);
}

async function getProvenanceVerifier() {
	defaultProvenanceVerifier ??= createProvenanceVerifier();
	return defaultProvenanceVerifier;
}

async function authenticateProvenance(bundle, verifyBundle) {
	try {
		await verifyBundle(bundle);
	} catch (error) {
		fail(`Sigstore provenance authentication failed: ${error?.message ?? String(error)}`);
	}
}

export async function reconcilePackage(pkg, identity, dependencies = {}) {
	const registry = dependencies.registry ?? ((args, description) => npmJson(args, description));
	const fetchJson = dependencies.fetchJson ?? fetchAttestationJson;
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
	if (candidates.length === 0) fail("attestation response must contain an SLSA provenance statement");
	if (candidates.length > 1) fail("attestation response must contain exactly one SLSA provenance statement");
	await authenticateProvenance(candidates[0]?.bundle, dependencies.verifyBundle ?? (await getProvenanceVerifier()));
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
	const allowPriorRelease = dependencies.allowPriorRelease ?? false;
	const provenanceRef = workflow?.ref;
	if (provenanceRef !== identity.ref && (!allowPriorRelease || !/^refs\/tags\/v\d+\.\d+\.\d+$/.test(provenanceRef))) {
		fail("SLSA workflow ref does not match");
	}
	const expectedUri = `git+${WORKFLOW_REPOSITORY}@${provenanceRef}`;
	const sources = build?.resolvedDependencies;
	if (!Array.isArray(sources) || sources.length !== 1 || sources[0]?.uri !== expectedUri) {
		fail("SLSA resolved source does not match the release tag");
	}
	const provenanceSha = sources[0]?.digest?.gitCommit;
	if (
		provenanceRef === identity.ref
			? provenanceSha !== identity.sha
			: !allowPriorRelease || !/^[0-9a-f]{40}$/.test(provenanceSha)
	) {
		fail("SLSA resolved commit does not match GITHUB_SHA");
	}
	return true;
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

export function isTransientReadError(error) {
	return (
		["AbortError", "TimeoutError"].includes(error?.name) ||
		["ETIMEDOUT", "ECONNRESET", "ECONNREFUSED", "EAI_AGAIN", "ENETUNREACH"].includes(error?.code) ||
		typeof error?.status === "number" ||
		/fetch failed|attestation request failed|provenance metadata must be an object|must have integrity and an attestation URL|must contain an SLSA provenance statement/.test(
			error?.message ?? "",
		)
	);
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
		if (error?.code === "ETIMEDOUT" || error?.signal) {
			fail(`npm publish for ${pkg.name}@${pkg.version} was interrupted; publication state is ambiguous and requires reconciliation`);
		}
		throw error;
	}
}

export async function reconcile(inventory, identity, dependencies = {}) {
	const verifyBundle = dependencies.verifyBundle ?? (await getProvenanceVerifier());
	const plan = preflight(inventory);
	for (const pkg of plan) {
		if (!pkg.present) {
			console.log(`Not yet published ${pkg.name}@${pkg.version}`);
			continue;
		}
		await reconcilePackage(pkg, identity, { ...dependencies, allowPriorRelease: true, verifyBundle });
		console.log(`Reconciled ${pkg.name}@${pkg.version}`);
	}
}

export async function publish(inventory, identity, dependencies = {}) {
	const verifyBundle = dependencies.verifyBundle ?? (await getProvenanceVerifier());
	const plan = preflight(inventory);
	for (const pkg of plan.filter(({ present }) => present)) {
		await reconcilePackage(pkg, identity, { ...dependencies, allowPriorRelease: true, verifyBundle });
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
		publishPackage(pkg);
		const publishedTags = await pollRead(
			`${pkg.name}@${pkg.version} registry visibility`,
			async () => {
				const publishedVersions = requireVersions(
					npmJson(["view", pkg.name, "versions", "--json"], `${pkg.name} versions after publish`),
					pkg.name,
				);
				if (!publishedVersions.includes(pkg.version)) fail(`${pkg.name}@${pkg.version} was not visible after publish`);
				const tags = requireDistTags(
					npmJson(["view", pkg.name, "dist-tags", "--json"], `${pkg.name} dist-tags after publish`),
					pkg.name,
				);
				if (tags.latest !== pkg.version) fail(`${pkg.name} latest did not move to ${pkg.version}`);
				return tags;
			},
			dependencies.pollDependencies,
		);
		const beforeNonLatest = { ...pkg.distTags };
		const afterNonLatest = { ...publishedTags };
		delete beforeNonLatest.latest;
		delete afterNonLatest.latest;
		if (!tagsEqual(beforeNonLatest, afterNonLatest)) fail(`${pkg.name} non-latest dist-tags changed during publish`);
		await pollRead(
			`${pkg.name}@${pkg.version} provenance`,
			() => reconcilePackage(pkg, identity, { ...dependencies, verifyBundle }),
			{ ...dependencies.pollDependencies, retryIf: isTransientReadError },
		);
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
	await reconcile(inventory, identity);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
	main().catch((error) => {
		console.error(`release: ${error.message}`);
		process.exitCode = 1;
	});
}
