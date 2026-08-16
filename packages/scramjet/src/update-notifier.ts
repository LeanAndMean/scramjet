import { readFileSync } from "node:fs";
import {
	CURRENT_RELEASE_TIMEOUT_MS,
	type ExtensionAPI,
	isCurrentInstallationManaged,
	isNewerPackageVersion,
	resolveCurrentRelease,
} from "@leanandmean/coding-agent";
import { packageRoot } from "./docs-registry.js";

const PACKAGE_NAME = "@leanandmean/scramjet";
export const UPDATE_CHECK_TIMEOUT_MS = CURRENT_RELEASE_TIMEOUT_MS;

export interface UpdateNotifierDependencies {
	installedVersion: () => string;
	isManagedInstallation: () => boolean;
}

function installedVersion(): string {
	try {
		const metadata: unknown = JSON.parse(readFileSync(`${packageRoot()}/package.json`, "utf8"));
		if (!metadata || typeof metadata !== "object") return "unknown";
		const { name, version } = metadata as { name?: unknown; version?: unknown };
		return name === PACKAGE_NAME && typeof version === "string" ? version : "unknown";
	} catch {
		return "unknown";
	}
}

const defaultDependencies: UpdateNotifierDependencies = {
	installedVersion,
	isManagedInstallation: isCurrentInstallationManaged,
};

function isTruthyEnvFlag(value: string | undefined): boolean {
	if (!value) return false;
	return value === "1" || value.toLowerCase() === "true" || value.toLowerCase() === "yes";
}

export function registerUpdateNotifier(
	pi: ExtensionAPI,
	dependencies: UpdateNotifierDependencies = defaultDependencies,
): void {
	let started = false;

	pi.on("session_start", (_event, ctx) => {
		if (
			!ctx.hasUI ||
			started ||
			isTruthyEnvFlag(process.env.SCRAMJET_OFFLINE) ||
			isTruthyEnvFlag(process.env.PI_OFFLINE)
		)
			return;
		started = true;

		void (async () => {
			try {
				const release = await resolveCurrentRelease(PACKAGE_NAME, pi.exec, UPDATE_CHECK_TIMEOUT_MS);
				const currentVersion = dependencies.installedVersion();
				if (!isNewerPackageVersion(release.version, currentVersion)) return;

				const guidance = dependencies.isManagedInstallation()
					? "Run `scramjet update` to update."
					: "Pull the latest source and reinstall from that checkout.";
				ctx.ui.notify(`Scramjet ${release.version} is available. ${guidance}`, "info");
			} catch {
				return;
			}
		})();
	});
}
