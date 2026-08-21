import { spawnSync } from "node:child_process";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const WORKFLOW_PATH = resolve(import.meta.dirname, "../../../.github/workflows/release.yml");
const source = readFileSync(WORKFLOW_PATH, "utf8");
const workflow = parse(source);
const steps = workflow.jobs.publish.steps as Array<Record<string, any>>;

function step(name: string) {
	return steps.find((candidate) => candidate.name === name)!;
}

function runRegistryValidation(overrides: Record<string, string> = {}) {
	const workDir = mkdtempSync(join(tmpdir(), "scramjet-registry-validation-"));
	try {
		writeFileSync(
			join(workDir, "npm"),
			`#!/bin/sh
case "$*" in
  "config get registry") echo "\${FAKE_REGISTRY:-https://registry.npmjs.org/}" ;;
  "config get @leanandmean:registry") echo "\${FAKE_SCOPE_REGISTRY:-undefined}" ;;
  "config get userconfig") echo "$HOME/user.npmrc" ;;
  "config get globalconfig") echo "$HOME/global.npmrc" ;;
  *) exit 2 ;;
esac
`,
		);
		chmodSync(join(workDir, "npm"), 0o755);
		const cleanEnv = Object.fromEntries(
			Object.entries(process.env).filter(
				([name, value]) =>
					value !== undefined &&
					!(/^(?:NPM|NODE).*TOKEN$/i.test(name) || /^NPM_CONFIG_.*(?:AUTH|PASSWORD|USERNAME)/i.test(name)),
			),
		);
		return spawnSync("bash", ["-euo", "pipefail", "-c", step("Validate registry configuration").run], {
			cwd: workDir,
			encoding: "utf8",
			env: {
				...cleanEnv,
				HOME: workDir,
				PATH: `${workDir}:${dirname(process.execPath)}:/usr/bin:/bin`,
				...overrides,
			},
		});
	} finally {
		rmSync(workDir, { recursive: true, force: true });
	}
}

describe("release workflow", () => {
	it("runs only for normal version tag pushes with global non-cancelling serialization", () => {
		expect(workflow.on).toEqual({ push: { tags: ["v[0-9]*"] } });
		expect(workflow.concurrency).toEqual({ group: "npm-publication", "cancel-in-progress": false });
	});

	it("uses exact least privilege, an event-ref checkout, and a bounded job", () => {
		expect(workflow.permissions).toEqual({ contents: "read", "id-token": "write" });
		expect(workflow.jobs.publish.permissions).toBeUndefined();
		expect(workflow.jobs.publish["timeout-minutes"]).toBe(30);
		const checkout = steps.find((candidate) => candidate.uses === "actions/checkout@v4")!;
		expect(checkout.with?.ref).toBeUndefined();
	});

	it("pins and verifies the release runtime", () => {
		expect(workflow.jobs.publish["runs-on"]).toBe("ubuntu-latest");
		const setup = steps.find((candidate) => candidate.uses === "actions/setup-node@v4")!;
		expect(setup.with["node-version"]).toBe("22");
		expect(step("Pin release tooling").run).toContain("Node 22.14.0 or newer is required");
		expect(step("Pin release tooling").run).toContain("npm install --global npm@11.5.1");
		expect(step("Pin release tooling").run).toContain("node --version");
		expect(step("Pin release tooling").run).toContain("npm --version");
	});

	it("validates identity and registry state before install, then builds before one helper publication", () => {
		const names = steps.map((candidate) => candidate.name);
		expect(names.indexOf("Validate release identity")).toBeLessThan(names.indexOf("Install dependencies"));
		expect(names.indexOf("Validate registry configuration")).toBeLessThan(names.indexOf("Install dependencies"));
		expect(names.indexOf("Build")).toBeLessThan(names.indexOf("Publish packages"));
		expect(step("Validate release identity").run).toBe("node .github/scripts/release.mjs validate");
		expect(step("Validate registry configuration").run).toContain("https://registry.npmjs.org/");
		expect(step("Validate registry configuration").run).toContain("npm config get @leanandmean:registry");
		expect(step("Validate registry configuration").run).toMatch(/_password\|username/);
		expect(step("Install dependencies").run).toBe("npm ci --ignore-scripts");
		expect(step("Build").run).toBe("npm run build");
		expect(step("Publish packages").run).toBe("node .github/scripts/release.mjs publish");
		expect(source.match(/release\.mjs publish/g)).toHaveLength(1);
		expect(source.match(/npm publish/g)).toBeNull();
	});

	it("executes registry and credential validation fail-closed", () => {
		expect(runRegistryValidation().status).toBe(0);
		expect(runRegistryValidation({ FAKE_REGISTRY: "https://example.test/" }).status).not.toBe(0);
		const credentials = runRegistryValidation({ NPM_TOKEN: "must-be-rejected" });
		expect(credentials.status).not.toBe(0);
		expect(credentials.stderr).toContain("npm credentials are present");
	});

	it("contains no token fallback or alternate dispatch path", () => {
		expect(source).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN|workflow_dispatch|npm@latest/);
	});
});
