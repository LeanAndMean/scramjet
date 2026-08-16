// SCRAMJET-DIVERGENCE: Replaced pi.dev release lookup with validated npm package resolution.

import { execFile } from "node:child_process";

export interface CurrentRelease {
	packageName: string;
	version: string;
}

export interface CurrentReleaseExecution {
	stdout: string;
	stderr: string;
	code: number;
	killed: boolean;
}

export type CurrentReleaseExecutor = (
	command: string,
	args: string[],
	options: { timeout: number },
) => Promise<CurrentReleaseExecution>;

export const CURRENT_RELEASE_TIMEOUT_MS = 5000;

interface ParsedVersion {
	major: string;
	minor: string;
	patch: string;
	prerelease?: string;
}

function parsePackageVersion(version: string): ParsedVersion | undefined {
	const match = version
		.trim()
		.match(
			/^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/,
		);
	if (!match || match[4]?.split(".").some((identifier) => /^0\d+$/.test(identifier))) {
		return undefined;
	}
	return {
		major: match[1],
		minor: match[2],
		patch: match[3],
		prerelease: match[4],
	};
}

function comparePackageVersions(leftVersion: string, rightVersion: string): number | undefined {
	const left = parsePackageVersion(leftVersion);
	const right = parsePackageVersion(rightVersion);
	if (!left || !right) {
		return undefined;
	}

	for (const component of ["major", "minor", "patch"] as const) {
		if (left[component] === right[component]) continue;
		if (left[component].length !== right[component].length) {
			return left[component].length - right[component].length;
		}
		return left[component].localeCompare(right[component]);
	}
	if (left.prerelease === right.prerelease) return 0;
	if (!left.prerelease) return 1;
	if (!right.prerelease) return -1;
	const leftIdentifiers = left.prerelease.split(".");
	const rightIdentifiers = right.prerelease.split(".");
	for (let index = 0; index < Math.max(leftIdentifiers.length, rightIdentifiers.length); index++) {
		const leftIdentifier = leftIdentifiers[index];
		const rightIdentifier = rightIdentifiers[index];
		if (leftIdentifier === undefined) return -1;
		if (rightIdentifier === undefined) return 1;
		if (leftIdentifier === rightIdentifier) continue;
		const leftNumeric = /^\d+$/.test(leftIdentifier);
		const rightNumeric = /^\d+$/.test(rightIdentifier);
		if (leftNumeric && rightNumeric) {
			if (leftIdentifier.length !== rightIdentifier.length) return leftIdentifier.length - rightIdentifier.length;
			return leftIdentifier.localeCompare(rightIdentifier);
		}
		if (leftNumeric) return -1;
		if (rightNumeric) return 1;
		return leftIdentifier.localeCompare(rightIdentifier);
	}
	return 0;
}

// SCRAMJET-DIVERGENCE: malformed package versions fail closed for passive update notifications (#432).
export function isNewerPackageVersion(candidateVersion: string, currentVersion: string): boolean {
	return (comparePackageVersions(candidateVersion, currentVersion) ?? 0) > 0;
}

const executeCurrentReleaseLookup: CurrentReleaseExecutor = (command, args, options) =>
	new Promise((resolve) => {
		execFile(command, args, { timeout: options.timeout }, (error, stdout, stderr) => {
			const processError = error as NodeJS.ErrnoException & { code?: number | string; killed?: boolean };
			resolve({
				stdout,
				stderr: stderr || error?.message || "",
				code: error ? (typeof processError.code === "number" ? processError.code : 1) : 0,
				killed: processError.killed === true,
			});
		});
	});

export async function resolveCurrentRelease(
	packageName: string,
	executor: CurrentReleaseExecutor = executeCurrentReleaseLookup,
	timeoutMs = CURRENT_RELEASE_TIMEOUT_MS,
): Promise<CurrentRelease> {
	const result = await executor("npm", ["view", packageName, "version", "--json"], { timeout: timeoutMs });
	if (result.killed) throw new Error(`npm release lookup timed out after ${timeoutMs}ms`);
	if (result.code !== 0) {
		const detail = result.stderr.trim();
		throw new Error(`npm release lookup failed with exit code ${result.code}${detail ? `: ${detail}` : ""}`);
	}

	let version: unknown;
	try {
		version = JSON.parse(result.stdout);
	} catch {
		throw new Error("npm release lookup returned malformed JSON");
	}
	if (typeof version !== "string" || !parsePackageVersion(version)) {
		throw new Error("npm release lookup returned an invalid package version");
	}
	return { packageName, version };
}
