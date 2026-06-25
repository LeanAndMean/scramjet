import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { initScramjet } from "../src/index.js";
import { discoverAgents } from "../src/subagent/agents.js";
import { getPiInvocation, registerSubagentTool } from "../src/subagent/index.js";
import { recordingPi } from "./helpers.js";

describe("registerSubagentTool — registration", () => {
	it("registers exactly one tool named 'subagent' with renderCall and renderResult", () => {
		const { pi, tools } = recordingPi();
		registerSubagentTool(pi);

		expect(tools).toHaveLength(1);
		expect(tools[0].name).toBe("subagent");
		expect(typeof tools[0].renderCall).toBe("function");
		expect(typeof tools[0].renderResult).toBe("function");
	});
});

describe("initScramjet — subagent wiring", () => {
	it("registers a tool named 'subagent' among all tools", () => {
		const { pi, tools } = recordingPi();
		initScramjet(pi);

		expect(tools.some((t: any) => t.name === "subagent")).toBe(true);
	});
});

describe("discoverAgents — empty directory", () => {
	let tmpDir: string;

	beforeEach(() => {
		tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "scramjet-agent-test-"));
	});

	afterEach(() => {
		fs.rmSync(tmpDir, { recursive: true, force: true });
	});

	it("returns empty agents array and null projectAgentsDir for a directory with no .scramjet/agents", () => {
		const result = discoverAgents(tmpDir, "project");

		expect(result.agents).toEqual([]);
		expect(result.projectAgentsDir).toBeNull();
	});
});

describe("getPiInvocation — fallback", () => {
	let origArgv1: string;
	let origExecPath: string;

	beforeEach(() => {
		origArgv1 = process.argv[1];
		origExecPath = process.execPath;
	});

	afterEach(() => {
		process.argv[1] = origArgv1;
		process.execPath = origExecPath;
	});

	it("falls back to 'scramjet' when argv[1] does not exist and execPath is a generic runtime", () => {
		process.argv[1] = "/nonexistent/path/to/script.js";
		process.execPath = "/usr/bin/node";

		const result = getPiInvocation(["--help"]);

		expect(result.command).toBe("scramjet");
		expect(result.args).toEqual(["--help"]);
	});
});
