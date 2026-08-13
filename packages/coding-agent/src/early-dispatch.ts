// SCRAMJET-DIVERGENCE: narrow product bootstrap routing avoids loading the interactive runtime.
import { handleConfigCommand, handlePackageCommand } from "./package-manager-cli.js";

export function isTruthyEnvFlag(value: string | undefined): boolean {
	if (!value) return false;
	return value === "1" || value.toLowerCase() === "true" || value.toLowerCase() === "yes";
}

export async function dispatchEarlyCliCommand(args: string[]): Promise<boolean> {
	const offlineMode =
		args.includes("--offline") ||
		isTruthyEnvFlag(process.env.SCRAMJET_OFFLINE) ||
		isTruthyEnvFlag(process.env.PI_OFFLINE);
	if (offlineMode) {
		process.env.SCRAMJET_OFFLINE = "1";
		process.env.PI_OFFLINE = "1";
		process.env.PI_SKIP_VERSION_CHECK = "1";
	}

	if (await handlePackageCommand(args)) return true;
	return handleConfigCommand(args);
}
