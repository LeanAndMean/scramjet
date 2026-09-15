import { createHash } from "node:crypto";
import { type Dirent, lstatSync, readdirSync, readFileSync, type Stats } from "node:fs";
import { isAbsolute, join, posix, resolve, win32 } from "node:path";

export type LegacyBundleScope = "global" | "project";

export interface LegacyInspectionError {
	code:
		| "top-level-symlink"
		| "top-level-not-directory"
		| "top-level-stat-failed"
		| "manifest-missing"
		| "manifest-invalid"
		| "manifest-read-failed"
		| "legacy-symlink"
		| "legacy-non-regular"
		| "legacy-stat-failed"
		| "legacy-read-failed"
		| "package-symlink"
		| "package-non-regular"
		| "package-stat-failed"
		| "package-read-failed"
		| "legacy-hash-failed"
		| "reserved-project-tree";
	path?: string;
}

export interface LegacyBundleInspection {
	legacyPath: string;
	scope: LegacyBundleScope;
	manifestVersion?: string;
	modified: string[];
	removed: string[];
	localOnly: string[];
	ambiguous: string[];
	errors: LegacyInspectionError[];
	actionable: boolean;
	signature: string;
}

export interface LegacyBundleInspectionOptions {
	legacyPath: string;
	packagePath: string;
	scope: LegacyBundleScope;
}

interface Manifest {
	version: string;
	files: Record<string, string>;
}

interface InspectionDependencies {
	orderEntries?: (entries: Dirent[]) => Dirent[];
}

const MANIFEST_NAME = ".seed-manifest.json";

function errorCode(error: unknown): string | undefined {
	return error && typeof error === "object" && "code" in error
		? String((error as NodeJS.ErrnoException).code)
		: undefined;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
	return (
		value !== null &&
		typeof value === "object" &&
		!Array.isArray(value) &&
		Object.getPrototypeOf(value) === Object.prototype
	);
}

function isSafeRelativePath(path: string): boolean {
	return (
		path.length > 0 &&
		!path.includes("\0") &&
		!path.includes("\\") &&
		!isAbsolute(path) &&
		!posix.isAbsolute(path) &&
		!win32.isAbsolute(path) &&
		path !== "." &&
		posix.normalize(path) === path &&
		path.split("/").every((part) => part !== "" && part !== "." && part !== "..")
	);
}

function parseManifest(value: unknown): Manifest | undefined {
	if (!isPlainObject(value) || typeof value.version !== "string" || !isPlainObject(value.files)) return undefined;
	const files = Object.create(null) as Record<string, string>;
	for (const [path, hash] of Object.entries(value.files)) {
		if (!isSafeRelativePath(path) || typeof hash !== "string" || !/^[a-f0-9]{64}$/i.test(hash)) return undefined;
		files[path] = hash.toLowerCase();
	}
	return { version: value.version, files };
}

function sortErrors(errors: LegacyInspectionError[]): LegacyInspectionError[] {
	return errors.sort((a, b) => a.code.localeCompare(b.code) || (a.path ?? "").localeCompare(b.path ?? ""));
}

function pathIsUnavailable(errors: LegacyInspectionError[], owner: "legacy" | "package", path: string): boolean {
	return errors.some(
		(error) =>
			error.code.startsWith(`${owner}-`) &&
			error.path !== undefined &&
			(path === error.path || path.startsWith(`${error.path}/`)),
	);
}

function walkRegularFiles(
	root: string,
	owner: "legacy" | "package",
	errors: LegacyInspectionError[],
	dependencies: InspectionDependencies,
): Map<string, string> {
	const files = new Map<string, string>();
	const walk = (dir: string, relativeDir: string) => {
		let entries: Dirent[];
		try {
			entries = readdirSync(dir, { withFileTypes: true });
		} catch {
			errors.push({ code: `${owner}-read-failed` as LegacyInspectionError["code"], path: relativeDir || undefined });
			return;
		}
		for (const entry of dependencies.orderEntries?.(entries) ?? entries) {
			const relativePath = relativeDir ? `${relativeDir}/${entry.name}` : entry.name;
			if (owner === "legacy" && relativePath === MANIFEST_NAME) continue;
			const absolutePath = join(dir, entry.name);
			let stat: Stats;
			try {
				stat = lstatSync(absolutePath);
			} catch {
				errors.push({ code: `${owner}-stat-failed` as LegacyInspectionError["code"], path: relativePath });
				continue;
			}
			if (stat.isSymbolicLink()) {
				errors.push({ code: `${owner}-symlink` as LegacyInspectionError["code"], path: relativePath });
				continue;
			}
			if (stat.isDirectory()) {
				walk(absolutePath, relativePath);
				continue;
			}
			if (stat.isFile()) files.set(relativePath, absolutePath);
			else errors.push({ code: `${owner}-non-regular` as LegacyInspectionError["code"], path: relativePath });
		}
	};
	walk(root, "");
	return files;
}

function finish(
	options: LegacyBundleInspectionOptions,
	manifestVersion: string | undefined,
	modified: string[],
	removed: string[],
	localOnly: string[],
	ambiguous: string[],
	errors: LegacyInspectionError[],
): LegacyBundleInspection {
	modified.sort();
	removed.sort();
	localOnly.sort();
	ambiguous.sort();
	sortErrors(errors);
	const actionable =
		modified.length > 0 || removed.length > 0 || localOnly.length > 0 || ambiguous.length > 0 || errors.length > 0;
	const signatureInput = JSON.stringify({
		legacyPath: resolve(options.legacyPath),
		scope: options.scope,
		manifestVersion,
		modified,
		removed,
		localOnly,
		ambiguous,
		errors,
	});
	return {
		legacyPath: options.legacyPath,
		scope: options.scope,
		manifestVersion,
		modified,
		removed,
		localOnly,
		ambiguous,
		errors,
		actionable,
		signature: createHash("sha256").update(signatureInput).digest("hex"),
	};
}

export function inspectLegacyBundle(
	options: LegacyBundleInspectionOptions,
	dependencies: InspectionDependencies = {},
): LegacyBundleInspection | undefined {
	let topLevel: Stats;
	try {
		topLevel = lstatSync(options.legacyPath);
	} catch (error) {
		if (errorCode(error) === "ENOENT") return undefined;
		return finish(options, undefined, [], [], [], [], [{ code: "top-level-stat-failed" }]);
	}
	if (topLevel.isSymbolicLink()) {
		return finish(options, undefined, [], [], [], [], [{ code: "top-level-symlink" }]);
	}
	if (!topLevel.isDirectory()) {
		return finish(options, undefined, [], [], [], [], [{ code: "top-level-not-directory" }]);
	}

	if (options.scope === "project") {
		const errors: LegacyInspectionError[] = [{ code: "reserved-project-tree" }];
		walkRegularFiles(options.legacyPath, "legacy", errors, dependencies);
		return finish(options, undefined, [], [], [], [], errors);
	}

	const manifestPath = join(options.legacyPath, MANIFEST_NAME);
	let manifestValue: unknown;
	try {
		const manifestStat = lstatSync(manifestPath);
		if (!manifestStat.isFile() || manifestStat.isSymbolicLink()) {
			return finish(options, undefined, [], [], [], [], [{ code: "manifest-invalid" }]);
		}
		manifestValue = JSON.parse(readFileSync(manifestPath, "utf8"));
	} catch (error) {
		if (errorCode(error) === "ENOENT") {
			return finish(options, undefined, [], [], [], [], [{ code: "manifest-missing" }]);
		}
		const code = error instanceof SyntaxError ? "manifest-invalid" : "manifest-read-failed";
		return finish(options, undefined, [], [], [], [], [{ code }]);
	}
	const manifest = parseManifest(manifestValue);
	if (!manifest) return finish(options, undefined, [], [], [], [], [{ code: "manifest-invalid" }]);

	const errors: LegacyInspectionError[] = [];
	const legacyFiles = walkRegularFiles(options.legacyPath, "legacy", errors, dependencies);
	const packageFiles = walkRegularFiles(options.packagePath, "package", errors, dependencies);
	const modified: string[] = [];
	const removed: string[] = [];
	const localOnly: string[] = [];
	const ambiguous: string[] = [];
	let hashFailed = false;

	for (const [path, expectedHash] of Object.entries(manifest.files)) {
		const filePath = legacyFiles.get(path);
		if (!filePath) {
			if (!pathIsUnavailable(errors, "legacy", path)) removed.push(path);
			continue;
		}
		try {
			const actualHash = createHash("sha256").update(readFileSync(filePath)).digest("hex");
			if (actualHash !== expectedHash) modified.push(path);
		} catch {
			errors.push({ code: "legacy-hash-failed", path });
			hashFailed = true;
		}
	}

	for (const path of legacyFiles.keys()) {
		if (Object.hasOwn(manifest.files, path)) continue;
		if (packageFiles.has(path)) ambiguous.push(path);
		else if (!pathIsUnavailable(errors, "package", path)) localOnly.push(path);
	}

	const readFailed = errors.some(
		(error) => error.code.endsWith("-read-failed") || error.code.endsWith("-stat-failed"),
	);
	if (hashFailed || readFailed) return finish(options, manifest.version, [], [], [], [], errors);
	return finish(options, manifest.version, modified, removed, localOnly, ambiguous, errors);
}

function paths(label: string, values: string[]): string | undefined {
	return values.length > 0 ? `${label}: ${values.join(", ")}.` : undefined;
}

function uncertainty(error: LegacyInspectionError): string {
	const path = error.path ? ` (${error.path})` : "";
	switch (error.code) {
		case "manifest-missing":
			return "seed manifest is missing; automatic classification is unavailable";
		case "manifest-invalid":
			return "seed manifest is malformed or unsafe; automatic classification is unavailable";
		case "manifest-read-failed":
			return "seed manifest could not be read; automatic classification is unavailable";
		case "reserved-project-tree":
			return "reserved project tree is explicitly user-created and cannot use a bundled namespace";
		case "top-level-symlink":
			return "top-level path is a symlink and its target was not inspected";
		case "top-level-not-directory":
			return "top-level path is not a directory";
		default:
			return `${error.code}${path}`;
	}
}

export function formatLegacyBundleWarning(inspection: LegacyBundleInspection): string {
	const proven = [
		paths("modified", inspection.modified),
		paths("removed", inspection.removed),
		paths("local-only", inspection.localOnly),
	].filter((value): value is string => Boolean(value));
	const uncertain = [
		paths("current-package collisions requiring manual comparison", inspection.ambiguous),
		...inspection.errors.map(uncertainty),
	];
	const evidence = [
		proven.length > 0 ? `Proven local changes: ${proven.join(" ")}` : undefined,
		uncertain.length > 0 ? `Uncertain: ${uncertain.join("; ")}. Compare manually before migration.` : undefined,
	]
		.filter((value): value is string => Boolean(value))
		.join(" ");
	return (
		`Ignored legacy bundled command set at ${inspection.legacyPath}; package resources are active and this path was left untouched. ` +
		`${evidence} To preserve customizations, fork under a different set name: rename command filenames and agent filenames, ` +
		`make agent frontmatter names match the new namespace, update delegation and next-step references, update references that ` +
		`invoke copied agents (references to uncopied package agents may remain), update applicable default keys, then reload ` +
		`Scramjet to validate agents. Scramjet command lint checks commands only; it does not validate agents.`
	);
}
