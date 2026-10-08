import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const WORKFLOW_PATH = resolve(import.meta.dirname, "../../../.github/workflows/release.yml");
const REGISTRY_GUARD = resolve(import.meta.dirname, "../../../.github/scripts/validate-registry.sh");
const source = readFileSync(WORKFLOW_PATH, "utf8");
const workflow = parse(source);
const native = parse(readFileSync(resolve(dirname(WORKFLOW_PATH), "terminal-feasibility.yml"), "utf8"));
const expression = (value: string) => `\${{ ${value} }}`;
const publishSteps = workflow.jobs.publish.steps as Array<Record<string, any>>;
const verifySteps = workflow.jobs.verify.steps as Array<Record<string, any>>;

function step(name: string, steps = publishSteps) {
	return steps.find((candidate) => candidate.name === name)!;
}

function runRegistryValidation({
	env = {},
	projectNpmrc,
	userNpmrc,
}: {
	env?: Record<string, string>;
	projectNpmrc?: string;
	userNpmrc?: string;
} = {}) {
	const workDir = mkdtempSync(join(tmpdir(), "scramjet-registry-validation-"));
	try {
		if (projectNpmrc !== undefined) writeFileSync(join(workDir, ".npmrc"), projectNpmrc);
		if (userNpmrc !== undefined) writeFileSync(join(workDir, "user.npmrc"), userNpmrc);
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
		return spawnSync("bash", [REGISTRY_GUARD], {
			cwd: workDir,
			encoding: "utf8",
			env: {
				...cleanEnv,
				HOME: workDir,
				PATH: `${workDir}:${dirname(process.execPath)}:/usr/bin:/bin`,
				...env,
			},
		});
	} finally {
		rmSync(workDir, { recursive: true, force: true });
	}
}

function runRegistryValidationWithNpm({ projectNpmrc, userNpmrc }: { projectNpmrc?: string; userNpmrc?: string }) {
	const root = mkdtempSync(join(tmpdir(), "scramjet-real-registry-validation-"));
	try {
		const project = join(root, "project");
		const home = join(root, "home");
		mkdirSync(project);
		mkdirSync(home);
		writeFileSync(join(root, "global.npmrc"), "");
		if (projectNpmrc !== undefined) writeFileSync(join(project, ".npmrc"), projectNpmrc);
		if (userNpmrc !== undefined) writeFileSync(join(home, ".npmrc"), userNpmrc);
		const cleanEnv = Object.fromEntries(
			Object.entries(process.env).filter(
				([name, value]) =>
					value !== undefined && !(/^NPM_CONFIG_/i.test(name) || /^(?:NPM|NODE).*TOKEN$/i.test(name)),
			),
		);
		return spawnSync("bash", [REGISTRY_GUARD], {
			cwd: project,
			encoding: "utf8",
			env: {
				...cleanEnv,
				HOME: home,
				NPM_CONFIG_GLOBALCONFIG: join(root, "global.npmrc"),
			},
		});
	} finally {
		rmSync(root, { recursive: true, force: true });
	}
}

describe("release workflow", () => {
	it("runs only for normal version tag pushes with global non-cancelling serialization", () => {
		expect(workflow.on).toEqual({ push: { tags: ["v[0-9]*"] } });
		expect(workflow.concurrency).toEqual({ group: "npm-publication", "cancel-in-progress": false });
	});

	it("separates publication privilege from dependent verification without changing trigger or concurrency", () => {
		expect(workflow.permissions).toEqual({ contents: "read" });
		expect(workflow.jobs.publish.permissions).toEqual({ contents: "read", "id-token": "write" });
		expect(workflow.jobs.verify.permissions).toBeUndefined();
		expect(workflow.jobs.publish["timeout-minutes"]).toBe(360);
		expect(workflow.jobs.verify["timeout-minutes"]).toBe(120);
		expect(workflow.jobs.verify.needs).toBe("publish");
		for (const steps of [publishSteps, verifySteps]) {
			const checkout = steps.find((candidate) => candidate.uses?.startsWith("actions/checkout@"))!;
			expect(checkout.uses).toBe("actions/checkout@11d5960a326750d5838078e36cf38b85af677262");
			expect(checkout.with?.ref).toBeUndefined();
			const setup = steps.find((candidate) => candidate.uses?.startsWith("actions/setup-node@"))!;
			expect(setup.uses).toBe("actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020");
			expect(setup.with["node-version"]).toBe("22");
			for (const action of steps.filter((candidate) => candidate.uses))
				expect(action.uses).toMatch(/@[0-9a-f]{40}$/);
		}
		expect(step("Validate registry configuration").run).toBe("bash .github/scripts/validate-registry.sh");
		expect(step("Validate registry configuration", verifySteps).run).toBe(
			"bash .github/scripts/validate-registry.sh",
		);
	});

	it("pins and verifies tooling in both jobs without a repository install in verification", () => {
		for (const [job, steps] of [
			[workflow.jobs.publish, publishSteps],
			[workflow.jobs.verify, verifySteps],
		] as const) {
			expect(job["runs-on"]).toBe("ubuntu-latest");
			const pin = step("Pin release tooling", steps).run;
			expect(pin).toContain("Node 22.14.0 or newer is required");
			expect(pin).toContain("npm install --global npm@11.5.1 --registry https://registry.npmjs.org/");
			expect(pin).toContain("node --version");
			expect(pin).toContain("npm --version");
		}
		expect(step("Pin release tooling", verifySteps).run).toBe(step("Pin release tooling").run);
		expect(
			verifySteps.find((candidate) => candidate.uses?.startsWith("actions/setup-node@"))!.with.cache,
		).toBeUndefined();
		expect(publishSteps.find((candidate) => candidate.uses?.startsWith("actions/setup-node@"))!.with.cache).toBe(
			"npm",
		);
	});

	it("preflights only the initial attempt and never builds, packs or publishes in the verify job", () => {
		for (const steps of [publishSteps, verifySteps]) {
			const names = steps.map((candidate) => candidate.name);
			expect(names.indexOf("Validate release identity")).toBeLessThan(
				names.indexOf("Validate registry configuration"),
			);
			expect(names.indexOf("Validate registry configuration")).toBeLessThan(names.indexOf("Pin release tooling"));
			expect(step("Validate release identity", steps).run).toBe("node .github/scripts/release.mjs validate");
		}
		const names = publishSteps.map((candidate) => candidate.name);
		expect(names.indexOf("Pin release tooling")).toBeLessThan(names.indexOf("Preflight release candidate"));
		expect(names.indexOf("Preflight release candidate")).toBeLessThan(names.indexOf("Install dependencies"));
		expect(names.indexOf("Install dependencies")).toBeLessThan(names.indexOf("Build"));
		expect(names.indexOf("Build")).toBeLessThan(names.indexOf("Publish packages"));
		expect(step("Preflight release candidate").if).toBe("github.run_attempt == 1");
		expect(step("Preflight release candidate").run).toBe('node .github/scripts/release.mjs preflight "$GITHUB_SHA"');
		expect(step("Install dependencies").run).toBe("npm ci --ignore-scripts");
		expect(step("Build").run).toBe("npm run build");
		expect(step("Publish packages").run).toBe("node .github/scripts/release.mjs publish");
		expect(verifySteps.map((candidate) => candidate.name)).toEqual([
			undefined,
			undefined,
			"Validate release identity",
			"Validate registry configuration",
			"Pin release tooling",
			"Verify published release",
		]);
		expect(step("Verify published release", verifySteps).run).toBe("node .github/scripts/release.mjs verify");
		expect(source.match(/release\.mjs publish/g)).toHaveLength(1);
		expect(source.match(/release\.mjs verify/g)).toHaveLength(1);
		expect(source).not.toMatch(/npm publish|upload-artifact|download-artifact/);
	});

	it("requires the same-revision native workflow before publication without granting it OIDC", () => {
		expect(workflow.jobs.native).toEqual({
			uses: "./.github/workflows/terminal-feasibility.yml",
			permissions: { contents: "read" },
		});
		expect(workflow.jobs.publish.needs).toBe("native");
		expect(workflow.jobs.publish.if).toBeUndefined();
		expect(native.on).toEqual({
			push: { branches: ["main"] },
			pull_request: { branches: ["main"] },
			workflow_dispatch: null,
			workflow_call: null,
		});
		expect(native.permissions).toEqual({ contents: "read" });
		for (const job of Object.values(native.jobs) as Array<Record<string, any>>) {
			expect(job.permissions).toBeUndefined();
			expect(job["continue-on-error"]).toBeUndefined();
			for (const entry of job.steps) {
				expect(entry["continue-on-error"]).toBeUndefined();
				if (entry.uses) expect(entry.uses).toMatch(/@[0-9a-f]{40}$/);
			}
		}
		for (const name of ["native-safety", "native-terminal"]) {
			const steps = native.jobs[name].steps;
			expect(steps[0].with).toEqual({ ref: expression("github.sha"), "persist-credentials": false });
			expect(steps[1].name).toBe("Require exact caller checkout");
			expect(steps[1].run).toContain('test "$(git rev-parse HEAD)" = "$GITHUB_SHA"');
			expect(steps[1].run).toContain('test -z "$(git status --porcelain)"');
		}
		const pack = native.jobs["native-terminal"].steps.find(
			(entry: any) => entry.name === "Pack isolated native candidate",
		);
		expect(pack.run).toContain('"$PACK_DIR/candidate.json"');
		expect(pack.run).toContain("process.env.GITHUB_SHA");
		expect(pack.run).toContain("installedRoot");
		expect(pack.if).toBe("runner.os == 'macOS'");
		expect(native.jobs["native-safety"].strategy.matrix.os).toEqual(["ubuntu-24.04", "macos-15"]);
	});

	it("runs installed, checkout and controls on every supported macOS terminal and architecture", () => {
		const job = native.jobs["native-terminal"];
		const rows = job.strategy.matrix.include as Array<Record<string, any>>;
		expect(rows).toEqual([
			{ os: "macos-15", terminal: "apple", tmux: false },
			{ os: "macos-15", terminal: "iterm2", tmux: false },
			{ os: "macos-15-intel", terminal: "apple", tmux: false },
			{ os: "macos-15-intel", terminal: "iterm2", tmux: false },
			{ os: "ubuntu-24.04", terminal: "vte", tmux: false },
			{ os: "ubuntu-24.04", terminal: "vte", tmux: true },
			{ os: "ubuntu-24.04", terminal: "xterm", tmux: false },
			{ os: "ubuntu-24.04", terminal: "kitty", tmux: false },
		]);
		const names = [
			"Pack isolated native candidate",
			"Probe native macOS terminal",
			"Probe checkout runtime",
			"Reject native false-positive controls",
		];
		const steps = names.map((name) => step(name, job.steps));
		for (const row of rows) {
			const mac = row.os.startsWith("macos-");
			for (const entry of steps) {
				expect(entry.if).toBe("runner.os == 'macOS'");
				const executes = entry.if === "runner.os == 'macOS'" && mac;
				expect(executes).toBe(["macos-15", "macos-15-intel"].includes(row.os));
			}
		}
		const [pack, installed, checkout, controls] = steps;
		const build = job.steps.find((entry: any) => entry.run === "npm run build");
		expect(build.if).toBeUndefined();
		expect(job.steps.indexOf(build)).toBeLessThan(job.steps.indexOf(pack));
		expect(steps.map((entry) => job.steps.indexOf(entry))).toEqual(
			steps.map((entry) => job.steps.indexOf(entry)).sort((a, b) => a - b),
		);
		expect(pack.run).toContain("for package in tui ai agent coding-agent scramjet");
		expect(pack.run).toContain('npm pack -w "packages/$package" --pack-destination "$PACK_DIR" --json');
		expect(pack.run).toContain('installed-runtime-smoke.mjs "$INSTALLED_ROOT"');
		expect(pack.run).toContain('echo "SCRAMJET_TUI_INSTALLED_ROOT=$INSTALLED_ROOT" >> "$GITHUB_ENV"');
		expect(installed.run).toContain(
			`terminal-probe.py "$RUNNER_TEMP/terminal-evidence" --terminal=${expression("matrix.terminal")}`,
		);
		expect(installed.run).not.toContain("env -u");
		expect(checkout.run).toContain("env -u SCRAMJET_TUI_INSTALLED_ROOT");
		expect(checkout.run).toContain(`--terminal=${expression("matrix.terminal")}`);
		expect(controls.run).toContain("set -euo pipefail");
		expect(controls.run).toContain("for control in noop-copy copy-on-selection consumed-paste");
		expect(controls.run).toContain(`--terminal=${expression("matrix.terminal")} --negative-control="$control"`);
		expect(controls.run).not.toContain("env -u");
		for (const entry of steps) expect(JSON.stringify(entry)).not.toMatch(/stock|--iterm-default-off/);
		const diagnostic = step("Diagnose iTerm2 default-off right-click", job.steps);
		expect(diagnostic.if).toBe("matrix.os == 'macos-15' && matrix.terminal == 'iterm2'");
		expect(diagnostic.run).toContain("--terminal=iterm2 --iterm-default-off");
		expect(diagnostic.run).toContain("terminal-evidence/default-off");
		expect(job.steps.indexOf(diagnostic)).toBeGreaterThan(job.steps.indexOf(controls));
		const linux = step("Probe Linux VTE terminal", job.steps);
		expect(linux.if).toBe("runner.os == 'Linux'");
		expect(linux.run).toContain("args+=(--tmux)");
		const artifacts = step("Preserve feasibility evidence", job.steps);
		expect(artifacts.if).toBe("always()");
		expect(artifacts.with.path).toContain("native-pack/*.json");
	});

	it("executes the native aggregate fail-closed for every non-success dependency result", () => {
		const gate = native.jobs["native-terminal-result"];
		expect(gate.if).toBe(expression("always()"));
		expect(gate.needs).toEqual(["native-safety", "native-terminal"]);
		const aggregate = gate.steps[0];
		expect(aggregate.env).toEqual({
			SAFETY_RESULT: expression("needs.native-safety.result"),
			INTERACTION_RESULT: expression("needs.native-terminal.result"),
		});
		for (const safety of ["success", "failure", "cancelled", "skipped", "", "malformed"]) {
			for (const interaction of ["success", "failure", "cancelled", "skipped", "", "malformed"]) {
				const result = spawnSync("bash", ["-e", "-c", aggregate.run], {
					env: { ...process.env, SAFETY_RESULT: safety, INTERACTION_RESULT: interaction },
				});
				expect(result.status === 0).toBe(safety === "success" && interaction === "success");
			}
		}
	});

	it("executes registry and credential validation fail-closed", () => {
		expect(runRegistryValidation().status).toBe(0);
		expect(runRegistryValidation({ env: { FAKE_REGISTRY: "https://example.test/" } }).status).not.toBe(0);
		expect(runRegistryValidation({ env: { FAKE_SCOPE_REGISTRY: "https://example.test/" } }).status).not.toBe(0);
		const environmentCredentials = runRegistryValidation({ env: { NPM_TOKEN: "must-be-rejected" } });
		expect(environmentCredentials.status).not.toBe(0);
		expect(environmentCredentials.stderr).toContain("npm credentials are present");
		const projectCredentials = runRegistryValidation({ projectNpmrc: "//registry.npmjs.org/:_authToken=secret\n" });
		expect(projectCredentials.status).not.toBe(0);
		expect(projectCredentials.stderr).toContain("npm credentials are present in .npmrc");
		const userCredentials = runRegistryValidation({ userNpmrc: "//registry.npmjs.org/:_authToken=secret\n" });
		expect(userCredentials.status).not.toBe(0);
		expect(userCredentials.stderr).toContain("npm credentials are present");
		expect(
			runRegistryValidationWithNpm({ projectNpmrc: "@leanandmean:registry=https://example.test/\n" }).status,
		).not.toBe(0);
		expect(
			runRegistryValidationWithNpm({ userNpmrc: "@leanandmean:registry=https://example.test/\n" }).status,
		).not.toBe(0);
		expect(
			runRegistryValidationWithNpm({ projectNpmrc: "@leanandmean:registry=https://registry.npmjs.org/\n" }).status,
		).toBe(0);
	});

	it("contains no token fallback or alternate dispatch path", () => {
		expect(source).not.toMatch(/NPM_TOKEN|NODE_AUTH_TOKEN|workflow_dispatch|npm@latest/);
	});
});
