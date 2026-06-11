import { existsSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { DOCS_REGISTRY, getDocPath } from "../docs-registry.ts";

describe("docs-registry", () => {
	it("all registered doc paths resolve to existing files", () => {
		for (const entry of DOCS_REGISTRY) {
			expect(existsSync(entry.path), `${entry.key}: ${entry.path} does not exist`).toBe(true);
		}
	});

	it("registry entries have non-empty labels and conditions", () => {
		for (const entry of DOCS_REGISTRY) {
			expect(entry.label.length, `${entry.key} has empty label`).toBeGreaterThan(0);
			expect(entry.condition.length, `${entry.key} has empty condition`).toBeGreaterThan(0);
		}
	});

	it("getDocPath returns the path for a known key", () => {
		expect(getDocPath("readme")).toMatch(/README\.md$/);
		expect(getDocPath("vision")).toMatch(/scramjet-vision\.md$/);
		expect(getDocPath("command-authoring")).toMatch(/command-authoring\.md$/);
	});

	it("getDocPath returns undefined for unknown keys", () => {
		expect(getDocPath("nonexistent")).toBeUndefined();
	});
});
