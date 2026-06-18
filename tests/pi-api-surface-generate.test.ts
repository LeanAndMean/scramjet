import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..");
const SCRIPT = join(REPO_ROOT, "scripts", "generate-pi-api-surface.js");
const OUTPUT = join(REPO_ROOT, "docs", "pi-api-surface.md");

function runGenerator(): string {
	return execFileSync(process.execPath, [SCRIPT], {
		cwd: REPO_ROOT,
		encoding: "utf-8",
	});
}

function readOutput(): string {
	return readFileSync(OUTPUT, "utf-8");
}

describe("scripts/generate-pi-api-surface.js", () => {
	let stdout: string;
	let output: string;

	beforeAll(() => {
		stdout = runGenerator();
		output = readOutput();
	}, 10_000);

	it("runs and produces non-empty output", () => {
		expect(stdout).toContain("Wrote docs/pi-api-surface.md");
		expect(output.length).toBeGreaterThan(1000);
		expect(output).toContain("# Pi API Surface");
	});

	it("includes all Pi package sections", () => {
		expect(output).toContain("## @earendil-works/pi-agent-core");
		expect(output).toContain("## @earendil-works/pi-ai");
		expect(output).toContain("## @earendil-works/pi-coding-agent");
		expect(output).toContain("## @earendil-works/pi-tui");
	});

	it("includes representative type signatures", () => {
		expect(output).toContain("#### ExtensionContext");
		expect(output).toContain("export interface ExtensionContext");
		expect(output).toContain("```ts");
	});

	it("excludes generated model catalog declarations", () => {
		expect(output).not.toContain("models.generated");
		expect(output).not.toContain("anthropic.claude-3-5-haiku-20241022-v1:0");
	});

	it("is deterministic across consecutive runs", () => {
		runGenerator();
		const first = readOutput();
		runGenerator();
		const second = readOutput();

		expect(second).toBe(first);
	}, 15_000);

	it("includes the Pi version header", () => {
		expect(output).toContain("Generated from Pi 0.74.0 / pi-coding-agent 0.74.0-scramjet.1.");
	});
});
