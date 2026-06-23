import { existsSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DOCS_REGISTRY, type DocKey, getDocPath } from "../src/docs-registry.js";

describe("docs-registry", () => {
	it("all registered doc paths resolve to existing files", () => {
		for (const entry of DOCS_REGISTRY) {
			expect(existsSync(entry.path), `${entry.key}: ${entry.path} does not exist`).toBe(true);
		}
	});

	it("registry entries have non-empty conditions", () => {
		for (const entry of DOCS_REGISTRY) {
			expect(entry.condition.length, `${entry.key} has empty condition`).toBeGreaterThan(0);
		}
	});

	it("registry has no duplicate keys", () => {
		const keys = DOCS_REGISTRY.map((e) => e.key);
		expect(new Set(keys).size).toBe(DOCS_REGISTRY.length);
	});

	it("getDocPath returns the path for a known key", () => {
		expect(getDocPath("readme")).toMatch(/README\.md$/);
		expect(getDocPath("vision")).toMatch(/scramjet-vision\.md$/);
		expect(getDocPath("command-authoring")).toMatch(/command-authoring\.md$/);
	});

	it("getDocPath is typed to accept only valid DocKeys", () => {
		const key: DocKey = "readme";
		expect(getDocPath(key)).toMatch(/README\.md$/);
	});
});
