// SCRAMJET-DIVERGENCE: Removed pi.dev network code (LATEST_VERSION_URL, fetch calls);
// renamed Pi-prefixed identifiers; network functions gutted to return undefined.

export interface LatestRelease {
	version: string;
	packageName?: string;
}

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

export async function getLatestRelease(
	_currentVersion: string,
	_options: { timeoutMs?: number } = {},
): Promise<LatestRelease | undefined> {
	return undefined;
}
