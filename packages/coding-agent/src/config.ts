import { spawnSync } from "child_process";
import { accessSync, constants, existsSync, lstatSync, readFileSync, readlinkSync, realpathSync, statSync } from "fs";
import { homedir } from "os";
import { basename, dirname, isAbsolute, join, relative, resolve, sep, win32 } from "path";
import { fileURLToPath } from "url";
import { shouldUseWindowsShell } from "./utils/child-process.js";

// =============================================================================
// Package Detection
// =============================================================================

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

/**
 * Detect if we're running as a Bun compiled binary.
 * Bun binaries have import.meta.url containing "$bunfs", "~BUN", or "%7EBUN" (Bun's virtual filesystem path)
 */
export const isBunBinary =
	import.meta.url.includes("$bunfs") || import.meta.url.includes("~BUN") || import.meta.url.includes("%7EBUN");

/** Detect if Bun is the runtime (compiled binary or bun run) */
export const isBunRuntime = !!process.versions.bun;

// =============================================================================
// Install Method Detection
// =============================================================================

export type InstallMethod = "bun-binary" | "npm" | "pnpm" | "yarn" | "bun" | "unknown";

interface SelfUpdateCommandStep {
	command: string;
	args: string[];
	display: string;
}

// SCRAMJET-DIVERGENCE: qualified product identity supports failure-safe npm self-updates.
export interface NpmRecoveryMetadata {
	readonly packageName: string;
	readonly productRoot: string;
	readonly packageRootType: "directory";
	readonly runtimeRoot: string;
	readonly manifestPath: string;
	readonly declaredBinPath: string;
	readonly binTargetPath: string;
	readonly launcherPath: string;
	readonly launcherType: "symbolic-link";
	readonly launcherLinkText: string;
	readonly launcherTargetPath: string;
	readonly productParentPath: string;
	readonly launcherParentPath: string;
	readonly productDevice: number;
	readonly productParentDevice: number;
	readonly launcherParentDevice: number;
	readonly layout: "npm-posix-product-tree";
}

export interface SelfUpdateCommand extends SelfUpdateCommandStep {
	steps?: SelfUpdateCommandStep[];
	readonly npmRecovery?: Readonly<NpmRecoveryMetadata>;
}

function makeSelfUpdateCommand(
	installStep: SelfUpdateCommandStep,
	uninstallStep?: SelfUpdateCommandStep,
): SelfUpdateCommand {
	if (!uninstallStep) return installStep;
	return {
		...installStep,
		display: `${uninstallStep.display} && ${installStep.display}`,
		steps: [uninstallStep, installStep],
	};
}

function makeSelfUpdateCommandStep(command: string, args: string[]): SelfUpdateCommandStep {
	return {
		command,
		args,
		display: [command, ...args].map((arg) => (/\s/.test(arg) ? `"${arg}"` : arg)).join(" "),
	};
}

export function detectInstallMethod(): InstallMethod {
	if (isBunBinary) {
		return "bun-binary";
	}

	const resolvedPath = `${__dirname}\0${process.execPath || ""}`.toLowerCase().replace(/\\/g, "/");

	if (resolvedPath.includes("/pnpm/") || resolvedPath.includes("/.pnpm/")) {
		return "pnpm";
	}
	if (resolvedPath.includes("/yarn/") || resolvedPath.includes("/.yarn/")) {
		return "yarn";
	}
	if (isBunRuntime || resolvedPath.includes("/install/global/node_modules/")) {
		return "bun";
	}
	if (resolvedPath.includes("/npm/") || resolvedPath.includes("/node_modules/")) {
		return "npm";
	}

	return "unknown";
}

function getInferredNpmInstall(): { root: string; prefix: string } | undefined {
	const packageDir = getPackageDir();
	const path = process.platform === "win32" || packageDir.includes("\\") ? win32 : { basename, dirname };
	const parent = path.dirname(packageDir);
	let root: string | undefined;
	if (path.basename(parent).startsWith("@") && path.basename(path.dirname(parent)) === "node_modules") {
		root = path.dirname(parent);
	} else if (path.basename(parent) === "node_modules") {
		root = parent;
	}
	if (!root) return undefined;
	const rootParent = path.dirname(root);
	if (path.basename(rootParent) === "lib") return { root, prefix: path.dirname(rootParent) };
	// Windows global npm prefixes use `<prefix>\\node_modules`, which is
	// indistinguishable from local project installs by path shape alone. Do not
	// infer unsupported Windows custom prefixes without `npm root -g` evidence.
	return undefined;
}

function getSelfUpdateCommandForMethod(
	method: InstallMethod,
	installedPackageName: string,
	updatePackageName = installedPackageName,
	npmCommand?: string[],
): SelfUpdateCommand | undefined {
	switch (method) {
		case "bun-binary":
			return undefined;
		case "pnpm":
			return makeSelfUpdateCommand(
				makeSelfUpdateCommandStep("pnpm", ["install", "-g", updatePackageName]),
				updatePackageName === installedPackageName
					? undefined
					: makeSelfUpdateCommandStep("pnpm", ["remove", "-g", installedPackageName]),
			);
		case "yarn":
			return makeSelfUpdateCommand(
				makeSelfUpdateCommandStep("yarn", ["global", "add", updatePackageName]),
				updatePackageName === installedPackageName
					? undefined
					: makeSelfUpdateCommandStep("yarn", ["global", "remove", installedPackageName]),
			);
		case "bun":
			return makeSelfUpdateCommand(
				makeSelfUpdateCommandStep("bun", ["install", "-g", updatePackageName]),
				updatePackageName === installedPackageName
					? undefined
					: makeSelfUpdateCommandStep("bun", ["uninstall", "-g", installedPackageName]),
			);
		case "npm": {
			const [command = "npm", ...npmArgs] = npmCommand ?? [];
			const inferred = npmCommand?.length ? undefined : getInferredNpmInstall();
			const prefixArgs = [...npmArgs, ...(inferred ? ["--prefix", inferred.prefix] : [])];
			const installStep = makeSelfUpdateCommandStep(command, [...prefixArgs, "install", "-g", updatePackageName]);
			const uninstallStep =
				updatePackageName === installedPackageName
					? undefined
					: makeSelfUpdateCommandStep(command, [...prefixArgs, "uninstall", "-g", installedPackageName]);
			return makeSelfUpdateCommand(installStep, uninstallStep);
		}
		case "unknown":
			return undefined;
	}
}

function readCommandOutput(
	command: string,
	args: string[],
	options: { requireSuccess?: boolean } = {},
): string | undefined {
	const result = spawnSync(command, args, {
		encoding: "utf-8",
		stdio: ["ignore", "pipe", "pipe"],
		shell: shouldUseWindowsShell(command),
	});
	if (result.status === 0) return result.stdout.trim() || undefined;
	if (options.requireSuccess) {
		const reason = result.error?.message || result.stderr.trim() || `exit code ${result.status ?? "unknown"}`;
		throw new Error(`Failed to run ${[command, ...args].join(" ")}: ${reason}`);
	}
	return undefined;
}

function getGlobalPackageRoots(method: InstallMethod, npmCommand?: string[]): string[] {
	switch (method) {
		case "npm": {
			const configured = !!npmCommand?.length;
			const [command = "npm", ...npmArgs] = npmCommand ?? [];
			if (configured && command === "bun") {
				const bunBin = readCommandOutput(command, [...npmArgs, "pm", "bin", "-g"], {
					requireSuccess: true,
				});
				const roots = [join(homedir(), ".bun", "install", "global", "node_modules")];
				if (bunBin) {
					roots.push(join(dirname(bunBin), "install", "global", "node_modules"));
				}
				return roots;
			}
			const root = readCommandOutput(command, [...npmArgs, "root", "-g"], {
				requireSuccess: configured,
			});
			const inferred = configured ? undefined : getInferredNpmInstall();
			return [root, inferred?.root].filter((x): x is string => !!x);
		}
		case "pnpm": {
			const root = readCommandOutput("pnpm", ["root", "-g"]);
			return root ? [root, dirname(root)] : [];
		}
		case "yarn": {
			const dir = readCommandOutput("yarn", ["global", "dir"]);
			return dir ? [dir, join(dir, "node_modules")] : [];
		}
		case "bun": {
			const bunBin = readCommandOutput("bun", ["pm", "bin", "-g"]);
			const roots = [join(homedir(), ".bun", "install", "global", "node_modules")];
			if (bunBin) {
				roots.push(join(dirname(bunBin), "install", "global", "node_modules"));
			}
			return roots;
		}
		case "bun-binary":
		case "unknown":
			return [];
	}
}

function normalizeExistingPathForComparison(path: string): string | undefined {
	const resolvedPath = resolve(path);
	if (!existsSync(resolvedPath)) {
		return undefined;
	}
	let normalizedPath: string;
	try {
		normalizedPath = realpathSync(resolvedPath);
	} catch {
		return undefined;
	}
	if (process.platform === "win32") {
		normalizedPath = normalizedPath.toLowerCase();
	}
	return normalizedPath;
}

function isSelfUpdatePathWritable(): boolean {
	const packageDir = getPackageDir();
	try {
		accessSync(packageDir, constants.W_OK);
		accessSync(dirname(packageDir), constants.W_OK);
		return true;
	} catch {
		return false;
	}
}

// SCRAMJET-DIVERGENCE: managed-install ownership is exposed independently of update writability (#432).
function isManagedByGlobalPackageManager(method: InstallMethod, npmCommand?: string[]): boolean {
	const packageDir = normalizeExistingPathForComparison(getPackageDir());
	return (
		!!packageDir &&
		getGlobalPackageRoots(method, npmCommand).some((root) => {
			const normalizedRoot = normalizeExistingPathForComparison(root);
			return (
				!!normalizedRoot &&
				packageDir.startsWith(normalizedRoot.endsWith(sep) ? normalizedRoot : `${normalizedRoot}${sep}`)
			);
		})
	);
}

export function isCurrentInstallationManaged(npmCommand?: string[]): boolean {
	return isManagedByGlobalPackageManager(detectInstallMethod(), npmCommand);
}

function findRuntimePackageRoot(): string | undefined {
	let directory = __dirname;
	while (directory !== dirname(directory)) {
		const manifestPath = join(directory, "package.json");
		if (existsSync(manifestPath)) {
			try {
				const manifest = JSON.parse(readFileSync(manifestPath, "utf-8")) as { name?: string };
				if (manifest.name === "@leanandmean/coding-agent") return realpathSync(directory);
			} catch {
				return undefined;
			}
		}
		directory = dirname(directory);
	}
	return undefined;
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

// SCRAMJET-DIVERGENCE: recovery is opt-in for the Stage 0-proven npm/POSIX product layout.
function qualifyNpmRecovery(
	packageName: string,
	updatePackageName: string,
	npmCommand?: string[],
): Readonly<NpmRecoveryMetadata> | undefined {
	if (
		(process.platform !== "linux" && process.platform !== "darwin") ||
		packageName !== updatePackageName ||
		npmCommand?.length
	) {
		return undefined;
	}
	const configuredProductRoot = process.env.SCRAMJET_INTERNAL_PRODUCT_ROOT;
	if (!configuredProductRoot) return undefined;

	try {
		const packageSegments = packageName.split("/");
		if (
			packageSegments.length !== 2 ||
			!packageSegments[0].startsWith("@") ||
			packageSegments.some((part) => !part || part === "." || part === "..")
		) {
			return undefined;
		}
		const productRootStat = lstatSync(configuredProductRoot);
		if (!productRootStat.isDirectory() || productRootStat.isSymbolicLink()) return undefined;
		const productRoot = realpathSync(configuredProductRoot);
		const runtimeRoot = findRuntimePackageRoot();
		if (!runtimeRoot || !isContainedPath(productRoot, runtimeRoot)) return undefined;
		const npmRoot = getGlobalPackageRoots("npm").find((root) => {
			const normalizedRoot = normalizeExistingPathForComparison(root);
			return normalizedRoot && join(normalizedRoot, ...packageSegments) === productRoot;
		});
		if (!npmRoot) return undefined;
		const normalizedNpmRoot = realpathSync(npmRoot);
		if (basename(normalizedNpmRoot) !== "node_modules" || basename(dirname(normalizedNpmRoot)) !== "lib") {
			return undefined;
		}

		const manifestPath = join(productRoot, "package.json");
		if (!lstatSync(manifestPath).isFile()) return undefined;
		const manifest = JSON.parse(readFileSync(manifestPath, "utf-8")) as {
			name?: string;
			bin?: Record<string, unknown>;
		};
		const declaredBinPath = manifest.bin?.scramjet;
		if (manifest.name !== packageName || typeof declaredBinPath !== "string") return undefined;
		const binTargetPath = resolve(productRoot, declaredBinPath);
		if (!isContainedPath(productRoot, binTargetPath) || !lstatSync(binTargetPath).isFile()) return undefined;
		if (realpathSync(binTargetPath) !== binTargetPath) return undefined;

		const prefix = dirname(dirname(normalizedNpmRoot));
		const launcherParentPath = join(prefix, "bin");
		const launcherPath = join(launcherParentPath, "scramjet");
		const launcherStat = lstatSync(launcherPath);
		if (!launcherStat.isSymbolicLink()) return undefined;
		const launcherLinkText = readlinkSync(launcherPath);
		const expectedLauncherLinkText = relative(launcherParentPath, binTargetPath);
		if (launcherLinkText !== expectedLauncherLinkText) return undefined;
		const launcherTargetPath = realpathSync(launcherPath);
		if (launcherTargetPath !== binTargetPath) return undefined;

		const productParentPath = dirname(productRoot);
		accessSync(productRoot, constants.W_OK);
		accessSync(productParentPath, constants.W_OK);
		accessSync(launcherParentPath, constants.W_OK);
		const productDevice = statSync(productRoot).dev;
		const productParentDevice = statSync(productParentPath).dev;
		const launcherParentDevice = statSync(launcherParentPath).dev;
		if (productDevice !== productParentDevice || statSync(binTargetPath).dev !== productDevice) return undefined;

		return Object.freeze({
			packageName,
			productRoot,
			packageRootType: "directory" as const,
			runtimeRoot,
			manifestPath,
			declaredBinPath,
			binTargetPath,
			launcherPath,
			launcherType: "symbolic-link" as const,
			launcherLinkText,
			launcherTargetPath,
			productParentPath,
			launcherParentPath,
			productDevice,
			productParentDevice,
			launcherParentDevice,
			layout: "npm-posix-product-tree" as const,
		});
	} catch {
		return undefined;
	}
}

export function getSelfUpdateCommand(
	packageName: string,
	npmCommand?: string[],
	updatePackageName = packageName,
): SelfUpdateCommand | undefined {
	const method = detectInstallMethod();
	const command = getSelfUpdateCommandForMethod(method, packageName, updatePackageName, npmCommand);
	if (!command || !isManagedByGlobalPackageManager(method, npmCommand) || !isSelfUpdatePathWritable()) {
		return undefined;
	}
	const npmRecovery = method === "npm" ? qualifyNpmRecovery(packageName, updatePackageName, npmCommand) : undefined;
	return npmRecovery ? { ...command, npmRecovery } : command;
}

export function getSelfUpdateUnavailableInstruction(
	packageName: string,
	npmCommand?: string[],
	updatePackageName = packageName,
): string {
	const method = detectInstallMethod();
	if (method === "bun-binary") {
		// SCRAMJET-DIVERGENCE: point to Scramjet releases, not upstream Pi.
		return `Download from: https://github.com/LeanAndMean/scramjet/releases/latest`;
	}
	const command = getSelfUpdateCommandForMethod(method, packageName, updatePackageName, npmCommand);
	if (command) {
		if (isManagedByGlobalPackageManager(method, npmCommand) && !isSelfUpdatePathWritable()) {
			return `This installation is managed by a global ${method} install, but the install path is not writable. Update it yourself with: ${command.display}`;
		}
		return `This installation is not managed by a global ${method} install. Update it with the package manager, wrapper, or source checkout that provides it.`;
	}
	return `Update ${updatePackageName} using the package manager, wrapper, or source checkout that provides this installation.`;
}

export function getUpdateInstruction(packageName: string): string {
	const method = detectInstallMethod();
	const command = getSelfUpdateCommandForMethod(method, packageName);
	if (command) {
		return `Run: ${command.display}`;
	}
	return getSelfUpdateUnavailableInstruction(packageName);
}

// =============================================================================
// Package Asset Paths (shipped with executable)
// =============================================================================

/**
 * Get the base directory for resolving package assets (themes, package.json, README.md, CHANGELOG.md).
 * - For Bun binary: returns the directory containing the executable
 * - For Node.js (dist/): returns __dirname (the dist/ directory)
 * - For tsx (src/): returns parent directory (the package root)
 */
export function getPackageDir(): string {
	// Allow override via environment variable (useful for Nix/Guix where store paths tokenize poorly)
	// SCRAMJET-DIVERGENCE: Prefer SCRAMJET_PACKAGE_DIR, fall back to PI_PACKAGE_DIR
	const envDir = process.env.SCRAMJET_PACKAGE_DIR || process.env.PI_PACKAGE_DIR;
	if (envDir) {
		if (envDir === "~") return homedir();
		if (envDir.startsWith("~/")) return homedir() + envDir.slice(1);
		return envDir;
	}

	if (isBunBinary) {
		// Bun binary: process.execPath points to the compiled executable
		return dirname(process.execPath);
	}
	// Node.js: walk up from __dirname until we find package.json
	let dir = __dirname;
	while (dir !== dirname(dir)) {
		if (existsSync(join(dir, "package.json"))) {
			return dir;
		}
		dir = dirname(dir);
	}
	// Fallback (shouldn't happen)
	return __dirname;
}

/**
 * Get path to built-in themes directory (shipped with package)
 * - For Bun binary: theme/ next to executable
 * - For Node.js (dist/): dist/modes/interactive/theme/
 * - For tsx (src/): src/modes/interactive/theme/
 */
export function getThemesDir(): string {
	if (isBunBinary) {
		return join(getPackageDir(), "theme");
	}
	// Theme is in modes/interactive/theme/ relative to src/ or dist/
	const packageDir = getPackageDir();
	const srcOrDist = existsSync(join(packageDir, "src")) ? "src" : "dist";
	return join(packageDir, srcOrDist, "modes", "interactive", "theme");
}

/**
 * Get path to HTML export template directory (shipped with package)
 * - For Bun binary: export-html/ next to executable
 * - For Node.js (dist/): dist/core/export-html/
 * - For tsx (src/): src/core/export-html/
 */
export function getExportTemplateDir(): string {
	if (isBunBinary) {
		return join(getPackageDir(), "export-html");
	}
	const packageDir = getPackageDir();
	const srcOrDist = existsSync(join(packageDir, "src")) ? "src" : "dist";
	return join(packageDir, srcOrDist, "core", "export-html");
}

/** Get path to package.json */
export function getPackageJsonPath(): string {
	return join(getPackageDir(), "package.json");
}

/** Get path to README.md */
export function getReadmePath(): string {
	return resolve(join(getPackageDir(), "README.md"));
}

/** Get path to docs directory */
export function getDocsPath(): string {
	return resolve(join(getPackageDir(), "docs"));
}

/** Get path to examples directory */
export function getExamplesPath(): string {
	return resolve(join(getPackageDir(), "examples"));
}

/** Get path to CHANGELOG.md */
export function getChangelogPath(): string {
	// SCRAMJET-DIVERGENCE: prefer SCRAMJET_CHANGELOG_PATH so the product displays
	// its own changelog instead of the runtime package's Pi upstream history.
	const envPath = process.env.SCRAMJET_CHANGELOG_PATH;
	if (envPath) return resolve(envPath);
	return resolve(join(getPackageDir(), "CHANGELOG.md"));
}

/**
 * Get path to built-in interactive assets directory.
 * - For Bun binary: assets/ next to executable
 * - For Node.js (dist/): dist/modes/interactive/assets/
 * - For tsx (src/): src/modes/interactive/assets/
 */
export function getInteractiveAssetsDir(): string {
	if (isBunBinary) {
		return join(getPackageDir(), "assets");
	}
	const packageDir = getPackageDir();
	const srcOrDist = existsSync(join(packageDir, "src")) ? "src" : "dist";
	return join(packageDir, srcOrDist, "modes", "interactive", "assets");
}

/** Get path to a bundled interactive asset */
export function getBundledInteractiveAssetPath(name: string): string {
	return join(getInteractiveAssetsDir(), name);
}

// =============================================================================
// App Config (from package.json piConfig)
// =============================================================================

interface PackageJson {
	name?: string;
	version?: string;
	piConfig?: {
		name?: string;
		configDir?: string;
	};
}

const pkg = JSON.parse(readFileSync(getPackageJsonPath(), "utf-8")) as PackageJson;

const piConfigName: string | undefined = pkg.piConfig?.name;
// SCRAMJET-DIVERGENCE: Scramjet self-updates the product package, not this runtime package.
export const PACKAGE_NAME: string = process.env.SCRAMJET_PACKAGE_NAME || pkg.name || "@leanandmean/coding-agent";
export const APP_NAME: string = piConfigName || "pi";
export const APP_TITLE: string = piConfigName ? APP_NAME : "π";
export const CONFIG_DIR_NAME: string = pkg.piConfig?.configDir || ".scramjet";
// SCRAMJET-DIVERGENCE: prefer SCRAMJET_VERSION env var so the product binary
// displays its own version rather than this runtime package's version.
export const VERSION: string = process.env.SCRAMJET_VERSION || pkg.version || "0.0.0";

// e.g., PI_CODING_AGENT_DIR or TAU_CODING_AGENT_DIR
export const ENV_AGENT_DIR = `${APP_NAME.toUpperCase()}_CODING_AGENT_DIR`;
export const ENV_SESSION_DIR = `${APP_NAME.toUpperCase()}_CODING_AGENT_SESSION_DIR`;

export function expandTildePath(path: string): string {
	if (path === "~") return homedir();
	if (path.startsWith("~/")) return homedir() + path.slice(1);
	return path;
}

// =============================================================================
// User Config Paths (~/.scramjet/agent/*)
// =============================================================================

/** Get the agent config directory (e.g., ~/.scramjet/agent/) */
export function getAgentDir(): string {
	const envDir = process.env[ENV_AGENT_DIR];
	if (envDir) {
		return expandTildePath(envDir);
	}
	return join(homedir(), CONFIG_DIR_NAME, "agent");
}

/** Get path to user's custom themes directory */
export function getCustomThemesDir(): string {
	return join(getAgentDir(), "themes");
}

/** Get path to models.json */
export function getModelsPath(): string {
	return join(getAgentDir(), "models.json");
}

/** Get path to auth.json */
export function getAuthPath(): string {
	return join(getAgentDir(), "auth.json");
}

/** Get path to settings.json */
export function getSettingsPath(): string {
	return join(getAgentDir(), "settings.json");
}

/** Get path to tools directory */
export function getToolsDir(): string {
	return join(getAgentDir(), "tools");
}

/** Get path to managed binaries directory (fd, rg) */
export function getBinDir(): string {
	return join(getAgentDir(), "bin");
}

/** Get path to prompt templates directory */
export function getPromptsDir(): string {
	return join(getAgentDir(), "prompts");
}

/** Get path to sessions directory */
export function getSessionsDir(): string {
	return join(getAgentDir(), "sessions");
}

/** Get path to debug log file */
export function getDebugLogPath(): string {
	return join(getAgentDir(), `${APP_NAME}-debug.log`);
}
