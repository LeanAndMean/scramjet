import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const PACKAGE_ROOT = resolve(HERE, "..");

function createLauncherFixture(): string {
	const productRoot = mkdtempSync(join(tmpdir(), "scramjet-launcher-bootstrap-"));
	const codingAgentRoot = join(productRoot, "node_modules", "@leanandmean", "coding-agent");
	mkdirSync(join(productRoot, "bin"), { recursive: true });
	mkdirSync(join(productRoot, "dist"), { recursive: true });
	mkdirSync(codingAgentRoot, { recursive: true });
	cpSync(join(PACKAGE_ROOT, "bin", "scramjet.js"), join(productRoot, "bin", "scramjet.js"));
	cpSync(join(PACKAGE_ROOT, "bin", "env-setup.js"), join(productRoot, "bin", "env-setup.js"));
	writeFileSync(
		join(productRoot, "package.json"),
		JSON.stringify({ name: "@leanandmean/scramjet", version: "1.2.3", type: "module" }),
	);
	writeFileSync(join(productRoot, "dist", "index.js"), "export const initScramjet = Symbol('product-init');\n");
	writeFileSync(
		join(codingAgentRoot, "package.json"),
		JSON.stringify({
			name: "@leanandmean/coding-agent",
			type: "module",
			exports: {
				".": "./index.js",
				"./early-dispatch": "./early-dispatch.js",
			},
		}),
	);
	writeFileSync(
		join(codingAgentRoot, "early-dispatch.js"),
		`export async function dispatchEarlyCliCommand(args) {
	if (process.env.SCRAMJET_PACKAGE_NAME !== "@leanandmean/scramjet") throw new Error("package env not initialized");
	if (process.env.SCRAMJET_INTERNAL_PRODUCT_ROOT !== ${JSON.stringify(productRoot)}) throw new Error("product root not initialized");
	if (args[0] === "update") { console.log("early update help"); return true; }
	return false;
}
`,
	);
	writeFileSync(
		join(codingAgentRoot, "index.js"),
		`export async function main(args, options) {
	if (args[0] !== "chat") throw new Error("unexpected ordinary args");
	if (typeof options?.builtinInit !== "symbol") throw new Error("product init not wired");
	console.log("ordinary runtime");
}
`,
	);
	return productRoot;
}

describe("product launcher bootstrap", () => {
	it("handles package commands before loading the full runtime or product", () => {
		const productRoot = createLauncherFixture();
		writeFileSync(
			join(productRoot, "node_modules", "@leanandmean", "coding-agent", "index.js"),
			"throw new Error('full runtime loaded');\n",
		);
		writeFileSync(join(productRoot, "dist", "index.js"), "throw new Error('product loaded');\n");

		const result = spawnSync(process.execPath, [join(productRoot, "bin", "scramjet.js"), "update", "--help"], {
			encoding: "utf-8",
			env: {
				...process.env,
				DISPLAY: ":launcher-boundary",
				SCRAMJET_INTERNAL_PRODUCT_ROOT: "/untrusted/product/root",
			},
		});

		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toBe("early update help\n");
		expect(result.stderr).toBe("");
	});

	it("loads and wires the ordinary runtime after early dispatch declines", () => {
		const productRoot = createLauncherFixture();

		const result = spawnSync(process.execPath, [join(productRoot, "bin", "scramjet.js"), "chat"], {
			encoding: "utf-8",
		});

		expect(result.status, result.stderr).toBe(0);
		expect(result.stdout).toBe("ordinary runtime\n");
		expect(result.stderr).toBe("");
	});
});
