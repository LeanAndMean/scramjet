import { describe, expect, it } from "vitest";
import { InMemorySettingsStorage, SettingsManager } from "../src/core/settings-manager.js";

describe("retry settings bounds", () => {
	it.each([-1, 0.5, "3", Infinity, NaN, Number.MAX_SAFE_INTEGER + 1])(
		"ignores invalid count %s without losing inherited provider settings",
		(value) => {
			const storage = new InMemorySettingsStorage();
			storage.withLock("global", () =>
				JSON.stringify({
					retry: { maxRetries: 2, provider: { maxRetries: 0, timeoutMs: 1234, maxRetryDelayMs: 5678 } },
				}),
			);
			storage.withLock("project", () =>
				JSON.stringify({ retry: { maxRetries: value, provider: { maxRetries: value } } }),
			);
			const manager = SettingsManager.fromStorage(storage);
			expect(manager.getRetrySettings().maxRetries).toBe(2);
			expect(manager.getProviderRetrySettings()).toEqual({ maxRetries: 0, timeoutMs: 1234, maxRetryDelayMs: 5678 });
			expect(manager.drainErrors().length).toBeGreaterThan(0);
		},
	);
	it("rejects JSON overflow, preserves zeros and diagnoses the project scope", () => {
		const storage = new InMemorySettingsStorage();
		storage.withLock(
			"global",
			() => '{"retry":{"maxRetries":0,"baseDelayMs":0,"provider":{"timeoutMs":0,"maxRetryDelayMs":0}}}',
		);
		storage.withLock(
			"project",
			() => '{"retry":{"maxRetries":1e309,"provider":{"timeoutMs":0.5,"maxRetryDelayMs":2147483648}}}',
		);
		const manager = SettingsManager.fromStorage(storage);
		expect(manager.getRetrySettings()).toEqual({ enabled: true, maxRetries: 0, baseDelayMs: 0 });
		expect(manager.getProviderRetrySettings()).toMatchObject({ timeoutMs: 0, maxRetryDelayMs: 0 });
		expect(manager.drainErrors().every((error) => error.scope === "project")).toBe(true);
		manager.reload();
		expect(manager.getRetrySettings().maxRetries).toBe(0);
	});
	it("validates programmatic overrides without mutating or erasing current values", () => {
		const manager = SettingsManager.inMemory({
			retry: { maxRetries: 2, provider: { maxRetries: 0, timeoutMs: 100 } },
		});
		const overrides = {
			retry: { maxRetries: Infinity, baseDelayMs: NaN, provider: { timeoutMs: 0.5, maxRetryDelayMs: 0 } },
		};
		manager.applyOverrides(overrides);
		expect(manager.getRetrySettings()).toEqual({ enabled: true, maxRetries: 2, baseDelayMs: 2000 });
		expect(manager.getProviderRetrySettings()).toEqual({ maxRetries: 0, timeoutMs: 100, maxRetryDelayMs: 0 });
		expect(overrides.retry.maxRetries).toBe(Infinity);
	});
});
