import type { SettingsManager } from "./settings-manager.js";

function isTruthyEnvFlag(value: string | undefined): boolean {
	if (!value) return false;
	return value === "1" || value.toLowerCase() === "true" || value.toLowerCase() === "yes";
}

// SCRAMJET-DIVERGENCE: prefer SCRAMJET_TELEMETRY, fall back to PI_TELEMETRY.
export function isInstallTelemetryEnabled(
	settingsManager: SettingsManager,
	telemetryEnv: string | undefined = process.env.SCRAMJET_TELEMETRY || process.env.PI_TELEMETRY,
): boolean {
	return telemetryEnv !== undefined ? isTruthyEnvFlag(telemetryEnv) : settingsManager.getEnableInstallTelemetry();
}
