import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { access, lstat, readFile, readlink, realpath, rename, rm, stat, symlink } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import type { NpmRecoveryMetadata, SelfUpdateCommand } from "./config.js";

const DEFAULT_PROBE_TIMEOUT_MS = 10_000;
const DEFAULT_PROBE_TERMINATION_GRACE_MS = 1_000;

export type NpmTransactionPhase =
	| "revalidation"
	| "evacuation"
	| "update-process"
	| "replacement-structure"
	| "replacement-probe"
	| "quarantine"
	| "package-restoration"
	| "launcher-restoration"
	| "restored-structure"
	| "restored-probe"
	| "cleanup";

export interface NpmTransactionFailure {
	readonly phase: NpmTransactionPhase;
	readonly error: Error;
	readonly path?: string;
}

export type NpmSelfUpdateOutcome =
	| {
			readonly status: "committed" | "committed-with-retained-artifacts";
			readonly cleanupFailures: readonly NpmTransactionFailure[];
			readonly retainedPaths: readonly string[];
	  }
	| {
			readonly status: "restored";
			readonly updateFailure: NpmTransactionFailure;
			readonly cleanupFailures: readonly NpmTransactionFailure[];
			readonly retainedPaths: readonly string[];
	  }
	| {
			readonly status: "restoration-unverified";
			readonly updateFailure: NpmTransactionFailure;
			readonly restorationFailures: readonly NpmTransactionFailure[];
			readonly retainedPaths: readonly string[];
	  };

export interface NpmSelfUpdateTransactionOptions {
	readonly probeTimeoutMs?: number;
	readonly probeTerminationGraceMs?: number;
	readonly transactionId?: string;
	readonly beforeCleanup?: (path: string) => Promise<void>;
	readonly removeArtifact?: (path: string) => Promise<void>;
}

interface TransactionPaths {
	backup: string;
	quarantine: string;
	temporaryLauncher: string;
}

interface ArtifactIdentity {
	path: string;
	dev: number;
	ino: number;
	type: "directory" | "file" | "symbolic-link" | "other";
}

interface ChildResult {
	code: number | null;
	signal: NodeJS.Signals | null;
	error?: Error;
	timedOut: boolean;
	terminationErrors: Error[];
}

function failure(phase: NpmTransactionPhase, error: unknown, path?: string): NpmTransactionFailure {
	return {
		phase,
		error: error instanceof Error ? error : new Error(String(error)),
		...(path ? { path } : {}),
	};
}

function isContainedPath(parent: string, child: string): boolean {
	const pathFromParent = relative(parent, child);
	return (
		pathFromParent !== "" &&
		pathFromParent !== ".." &&
		!pathFromParent.startsWith(`..${sep}`) &&
		!isAbsolute(pathFromParent)
	);
}

async function pathExists(path: string): Promise<boolean> {
	try {
		await lstat(path);
		return true;
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") return false;
		throw error;
	}
}

function artifactType(stats: Awaited<ReturnType<typeof lstat>>): ArtifactIdentity["type"] {
	if (stats.isSymbolicLink()) return "symbolic-link";
	if (stats.isDirectory()) return "directory";
	if (stats.isFile()) return "file";
	return "other";
}

async function identifyArtifact(path: string): Promise<ArtifactIdentity> {
	const stats = await lstat(path);
	return { path, dev: stats.dev, ino: stats.ino, type: artifactType(stats) };
}

async function validateArtifactIdentity(identity: ArtifactIdentity): Promise<void> {
	const current = await identifyArtifact(identity.path);
	if (current.dev !== identity.dev || current.ino !== identity.ino || current.type !== identity.type) {
		throw new Error(`Cleanup target identity changed: ${identity.path}`);
	}
}

async function readManifest(
	metadata: Readonly<NpmRecoveryMetadata>,
): Promise<{ name?: string; bin?: Record<string, unknown> }> {
	const manifest = JSON.parse(await readFile(metadata.manifestPath, "utf8")) as {
		name?: string;
		bin?: Record<string, unknown>;
	};
	return manifest;
}

async function validateStructure(metadata: Readonly<NpmRecoveryMetadata>): Promise<void> {
	if (dirname(metadata.manifestPath) !== metadata.productRoot) {
		throw new Error("Manifest path is outside the canonical product root");
	}
	if (dirname(metadata.launcherPath) !== metadata.launcherParentPath) {
		throw new Error("Launcher path is outside its qualified parent");
	}
	if (dirname(metadata.productRoot) !== metadata.productParentPath) {
		throw new Error("Product path is outside its qualified parent");
	}
	if (!isContainedPath(metadata.productRoot, metadata.binTargetPath)) {
		throw new Error("Declared bin target escapes the canonical product root");
	}

	const productStats = await lstat(metadata.productRoot);
	if (!productStats.isDirectory() || productStats.isSymbolicLink()) {
		throw new Error("Canonical product root is not a real directory");
	}
	if ((await realpath(metadata.productRoot)) !== metadata.productRoot) {
		throw new Error("Canonical product root changed identity");
	}
	const manifestStats = await lstat(metadata.manifestPath);
	if (!manifestStats.isFile() || manifestStats.isSymbolicLink()) {
		throw new Error("Product manifest is not a real file");
	}
	const manifest = await readManifest(metadata);
	if (manifest.name !== metadata.packageName) {
		throw new Error(`Unexpected replacement package name: ${manifest.name ?? "missing"}`);
	}
	if (manifest.bin?.scramjet !== metadata.declaredBinPath) {
		throw new Error("Replacement bin.scramjet declaration changed");
	}
	if (resolve(metadata.productRoot, metadata.declaredBinPath) !== metadata.binTargetPath) {
		throw new Error("Replacement bin.scramjet target changed");
	}
	const binStats = await lstat(metadata.binTargetPath);
	if (!binStats.isFile() || binStats.isSymbolicLink()) {
		throw new Error("Replacement bin target is not a real file");
	}
	if ((await realpath(metadata.binTargetPath)) !== metadata.binTargetPath) {
		throw new Error("Replacement bin target changed identity");
	}
	const launcherStats = await lstat(metadata.launcherPath);
	if (!launcherStats.isSymbolicLink()) {
		throw new Error("Canonical launcher is not a symbolic link");
	}
	if ((await readlink(metadata.launcherPath)) !== metadata.launcherLinkText) {
		throw new Error("Canonical launcher link text changed");
	}
	if ((await realpath(metadata.launcherPath)) !== metadata.launcherTargetPath) {
		throw new Error("Canonical launcher target changed");
	}
}

async function revalidateBeforeEvacuation(
	metadata: Readonly<NpmRecoveryMetadata>,
	paths: TransactionPaths,
): Promise<void> {
	if (metadata.layout !== "npm-posix-product-tree") throw new Error("Unsupported recovery layout");
	await validateStructure(metadata);
	if (!isContainedPath(metadata.productRoot, metadata.runtimeRoot)) {
		throw new Error("Qualified runtime root escapes the product tree");
	}
	const runtimeStats = await lstat(metadata.runtimeRoot);
	if (!runtimeStats.isDirectory() || runtimeStats.isSymbolicLink()) {
		throw new Error("Qualified runtime root changed identity");
	}
	await access(metadata.productRoot, constants.W_OK);
	await access(metadata.productParentPath, constants.W_OK);
	await access(metadata.launcherParentPath, constants.W_OK);
	if ((await stat(metadata.productRoot)).dev !== metadata.productDevice) throw new Error("Product filesystem changed");
	if ((await stat(metadata.productParentPath)).dev !== metadata.productParentDevice) {
		throw new Error("Product parent filesystem changed");
	}
	if ((await stat(metadata.launcherParentPath)).dev !== metadata.launcherParentDevice) {
		throw new Error("Launcher parent filesystem changed");
	}
	if (metadata.productDevice !== metadata.productParentDevice)
		throw new Error("Product backup is not same-filesystem");
	for (const path of [paths.backup, paths.quarantine, paths.temporaryLauncher]) {
		if (await pathExists(path)) throw new Error(`Transaction path already exists: ${path}`);
	}
}

function transactionPaths(metadata: Readonly<NpmRecoveryMetadata>, id: string): TransactionPaths {
	if (!/^[A-Za-z0-9-]+$/.test(id)) throw new Error("Invalid transaction identifier");
	const productName = basename(metadata.productRoot);
	return {
		backup: join(metadata.productParentPath, `.${productName}.scramjet-backup-${id}`),
		quarantine: join(metadata.productParentPath, `.${productName}.scramjet-quarantine-${id}`),
		temporaryLauncher: join(metadata.launcherParentPath, `.scramjet-launcher-${id}`),
	};
}

function runChild(
	command: string,
	args: string[],
	options: { stdio: "inherit" | "ignore"; timeoutMs?: number; terminationGraceMs?: number },
): Promise<ChildResult> {
	return new Promise((resolveResult) => {
		const child = spawn(command, args, { stdio: options.stdio, shell: false });
		let spawnError: Error | undefined;
		let timedOut = false;
		const terminationErrors: Error[] = [];
		let timeout: NodeJS.Timeout | undefined;
		let escalation: NodeJS.Timeout | undefined;

		const clearTimers = () => {
			if (timeout) clearTimeout(timeout);
			if (escalation) clearTimeout(escalation);
		};
		child.once("error", (error) => {
			spawnError = error;
		});
		child.once("close", (code, signal) => {
			clearTimers();
			resolveResult({ code, signal, error: spawnError, timedOut, terminationErrors });
		});

		if (options.timeoutMs !== undefined) {
			timeout = setTimeout(() => {
				timedOut = true;
				try {
					if (!child.kill("SIGTERM")) terminationErrors.push(new Error("Probe process rejected SIGTERM"));
				} catch (error) {
					terminationErrors.push(error instanceof Error ? error : new Error(String(error)));
				}
				escalation = setTimeout(() => {
					try {
						if (!child.kill("SIGKILL")) terminationErrors.push(new Error("Probe process rejected SIGKILL"));
					} catch (error) {
						terminationErrors.push(error instanceof Error ? error : new Error(String(error)));
					}
				}, options.terminationGraceMs ?? DEFAULT_PROBE_TERMINATION_GRACE_MS);
			}, options.timeoutMs);
		}
	});
}

function processFailure(result: ChildResult, display: string): Error | undefined {
	if (result.error) return result.error;
	if (result.timedOut) {
		const termination = result.terminationErrors.map((error) => error.message).join("; ");
		return new Error(`${display} timed out${termination ? ` (${termination})` : ""}`);
	}
	if (result.signal) return new Error(`${display} terminated by signal ${result.signal}`);
	if (result.code !== 0) return new Error(`${display} exited with code ${result.code ?? "unknown"}`);
	if (result.terminationErrors.length > 0) return result.terminationErrors[0];
	return undefined;
}

async function runProbe(
	metadata: Readonly<NpmRecoveryMetadata>,
	options: NpmSelfUpdateTransactionOptions,
): Promise<void> {
	const result = await runChild(metadata.launcherPath, ["--help"], {
		stdio: "ignore",
		timeoutMs: options.probeTimeoutMs ?? DEFAULT_PROBE_TIMEOUT_MS,
		terminationGraceMs: options.probeTerminationGraceMs ?? DEFAULT_PROBE_TERMINATION_GRACE_MS,
	});
	const error = processFailure(result, "Canonical launcher probe");
	if (error) throw error;
}

async function cleanupArtifact(
	identity: ArtifactIdentity,
	removeArtifact: (path: string) => Promise<void>,
	beforeCleanup?: (path: string) => Promise<void>,
): Promise<NpmTransactionFailure | undefined> {
	try {
		await beforeCleanup?.(identity.path);
		await validateArtifactIdentity(identity);
		await removeArtifact(identity.path);
		return undefined;
	} catch (error) {
		return failure("cleanup", error, identity.path);
	}
}

async function retainedPaths(paths: readonly string[]): Promise<string[]> {
	const retained: string[] = [];
	for (const path of paths) {
		if (await pathExists(path)) retained.push(path);
	}
	return retained;
}

export async function runNpmSelfUpdateTransaction(
	command: SelfUpdateCommand,
	metadata: Readonly<NpmRecoveryMetadata>,
	options: NpmSelfUpdateTransactionOptions = {},
): Promise<NpmSelfUpdateOutcome> {
	const qualifiedMetadata = metadata;
	metadata = Object.freeze({ ...metadata });
	const id = options.transactionId ?? `${process.pid}-${randomUUID()}`;
	let paths: TransactionPaths;
	try {
		paths = transactionPaths(metadata, id);
		if (command.steps || command.npmRecovery !== qualifiedMetadata) {
			throw new Error("Qualified npm recovery requires one matching update step");
		}
		await revalidateBeforeEvacuation(metadata, paths);
	} catch (error) {
		return {
			status: "restoration-unverified",
			updateFailure: failure("revalidation", error),
			restorationFailures: [],
			retainedPaths: [],
		};
	}

	let productIdentity: ArtifactIdentity;
	try {
		productIdentity = await identifyArtifact(metadata.productRoot);
		await rename(metadata.productRoot, paths.backup);
	} catch (error) {
		return {
			status: "restoration-unverified",
			updateFailure: failure("evacuation", error, paths.backup),
			restorationFailures: [],
			retainedPaths: await retainedPaths([paths.backup, paths.quarantine, paths.temporaryLauncher]),
		};
	}
	const backupIdentity = { ...productIdentity, path: paths.backup };

	let updateFailure: NpmTransactionFailure | undefined;
	const updateResult = await runChild(command.command, command.args, { stdio: "inherit" });
	const updateError = processFailure(updateResult, command.display);
	if (updateError) {
		updateFailure = failure("update-process", updateError);
	} else {
		try {
			await validateStructure(metadata);
		} catch (error) {
			updateFailure = failure("replacement-structure", error);
		}
		if (!updateFailure) {
			try {
				await runProbe(metadata, options);
			} catch (error) {
				updateFailure = failure("replacement-probe", error);
			}
		}
	}

	const removeArtifact = options.removeArtifact ?? ((path: string) => rm(path, { recursive: true, force: true }));
	if (!updateFailure) {
		const cleanupFailure = await cleanupArtifact(backupIdentity, removeArtifact, options.beforeCleanup);
		const retained = await retainedPaths([paths.backup]);
		return {
			status: cleanupFailure || retained.length > 0 ? "committed-with-retained-artifacts" : "committed",
			cleanupFailures: cleanupFailure ? [cleanupFailure] : [],
			retainedPaths: retained,
		};
	}

	const restorationFailures: NpmTransactionFailure[] = [];
	let quarantineIdentity: ArtifactIdentity | undefined;
	let quarantineTrackingFailure: NpmTransactionFailure | undefined;
	let canonicalIdentity: ArtifactIdentity | undefined;
	let canonicalExists = true;
	try {
		canonicalIdentity = await identifyArtifact(metadata.productRoot);
	} catch (error) {
		if ((error as NodeJS.ErrnoException).code === "ENOENT") {
			canonicalExists = false;
		} else {
			quarantineTrackingFailure = failure("cleanup", error, paths.quarantine);
		}
	}
	if (canonicalExists) {
		try {
			await rename(metadata.productRoot, paths.quarantine);
			if (canonicalIdentity) quarantineIdentity = { ...canonicalIdentity, path: paths.quarantine };
		} catch (error) {
			restorationFailures.push(failure("quarantine", error, paths.quarantine));
		}
	}

	if (restorationFailures.length === 0) {
		try {
			await validateArtifactIdentity(backupIdentity);
			await rename(paths.backup, metadata.productRoot);
		} catch (error) {
			restorationFailures.push(failure("package-restoration", error, paths.backup));
		}
	}
	if (restorationFailures.length === 0) {
		try {
			await symlink(metadata.launcherLinkText, paths.temporaryLauncher);
			await rename(paths.temporaryLauncher, metadata.launcherPath);
		} catch (error) {
			restorationFailures.push(failure("launcher-restoration", error, paths.temporaryLauncher));
		}
	}
	if (restorationFailures.length === 0) {
		try {
			await validateStructure(metadata);
		} catch (error) {
			restorationFailures.push(failure("restored-structure", error));
		}
	}
	if (restorationFailures.length === 0) {
		try {
			await runProbe(metadata, options);
		} catch (error) {
			restorationFailures.push(failure("restored-probe", error));
		}
	}

	if (restorationFailures.length > 0) {
		return {
			status: "restoration-unverified",
			updateFailure,
			restorationFailures,
			retainedPaths: await retainedPaths([paths.backup, paths.quarantine, paths.temporaryLauncher]),
		};
	}

	const cleanupFailures: NpmTransactionFailure[] = quarantineTrackingFailure ? [quarantineTrackingFailure] : [];
	if (quarantineIdentity) {
		const cleanupFailure = await cleanupArtifact(quarantineIdentity, removeArtifact, options.beforeCleanup);
		if (cleanupFailure) cleanupFailures.push(cleanupFailure);
	}
	return {
		status: "restored",
		updateFailure,
		cleanupFailures,
		retainedPaths: await retainedPaths([paths.quarantine]),
	};
}
