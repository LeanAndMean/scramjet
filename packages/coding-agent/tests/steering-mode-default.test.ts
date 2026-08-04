import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { AuthStorage } from "../src/core/auth-storage.js";
import { ModelRegistry } from "../src/core/model-registry.js";
import { createAgentSession } from "../src/core/sdk.js";
import { SessionManager } from "../src/core/session-manager.js";
import { SettingsManager } from "../src/core/settings-manager.js";

const temporaryDirectories: string[] = [];

afterEach(() => {
	for (const directory of temporaryDirectories.splice(0)) {
		rmSync(directory, { recursive: true, force: true });
	}
});

describe("steering mode defaults", () => {
	it("defaults steering to all while preserving the follow-up default", () => {
		const settings = SettingsManager.inMemory();

		expect(settings.getSteeringMode()).toBe("all");
		expect(settings.getFollowUpMode()).toBe("one-at-a-time");
	});

	it.each(["all", "one-at-a-time"] as const)("preserves an explicit %s steering mode", (mode) => {
		expect(SettingsManager.inMemory({ steeringMode: mode }).getSteeringMode()).toBe(mode);
	});

	it("applies the defaults to a normally constructed session", async () => {
		const root = mkdtempSync(join(tmpdir(), "steering-mode-default-"));
		temporaryDirectories.push(root);
		const cwd = join(root, "cwd");
		const agentDir = join(root, "agent");
		const authStorage = AuthStorage.inMemory();
		const settingsManager = SettingsManager.inMemory();
		const { session } = await createAgentSession({
			cwd,
			agentDir,
			authStorage,
			modelRegistry: ModelRegistry.inMemory(authStorage),
			sessionManager: SessionManager.inMemory(cwd),
			settingsManager,
		});

		try {
			expect(session.steeringMode).toBe("all");
			expect(session.followUpMode).toBe("one-at-a-time");
		} finally {
			session.dispose();
		}
	});
});
