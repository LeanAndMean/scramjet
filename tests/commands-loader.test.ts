import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { registerCommandLoader } from "../commands/index.ts";
import { buildRegistry, type FileEntry, parseAllowedTools, parseCommandFile } from "../commands/loader.ts";
import { freshState } from "./helpers.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const FIXTURES = resolve(HERE, "fixtures");

type Handler = (event: unknown, ctx?: unknown) => unknown;

function recordingPi() {
	const handlers = new Map<string, Handler[]>();
	const pi: any = {
		on(event: string, handler: Handler) {
			const list = handlers.get(event) ?? [];
			list.push(handler);
			handlers.set(event, list);
		},
	};
	return { pi, handlers };
}

describe("parseAllowedTools — input shapes", () => {
	it("returns undefined for absent values", () => {
		expect(parseAllowedTools(undefined)).toBeUndefined();
		expect(parseAllowedTools(null)).toBeUndefined();
	});

	it("returns the trimmed items from a YAML array", () => {
		expect(parseAllowedTools(["Read", " Bash ", "Edit"])).toEqual(["Read", "Bash", "Edit"]);
	});

	it("returns the trimmed items from a comma-separated string", () => {
		expect(parseAllowedTools("Read, Bash , Edit")).toEqual(["Read", "Bash", "Edit"]);
	});

	it("drops non-string array entries silently", () => {
		expect(parseAllowedTools(["Read", 42, null, "Edit"])).toEqual(["Read", "Edit"]);
	});

	it("returns undefined when an array contains only empty/non-string entries", () => {
		expect(parseAllowedTools(["", "  "])).toBeUndefined();
		expect(parseAllowedTools([42, null])).toBeUndefined();
	});

	it("returns undefined for empty string", () => {
		expect(parseAllowedTools("")).toBeUndefined();
		expect(parseAllowedTools("  ,  ")).toBeUndefined();
	});

	it("returns undefined for invalid scalar types", () => {
		expect(parseAllowedTools(42)).toBeUndefined();
		expect(parseAllowedTools({})).toBeUndefined();
	});
});

describe("parseCommandFile — happy paths", () => {
	const SET = "mach12";

	it("parses a valid file into a CommandDef", () => {
		const content = `---
description: A demo command.
allowed-tools: [Read, Bash]
next:
  mode: ask
  hint: User picks next
---
Body text.`;
		const result = parseCommandFile("/abs/mach12:issue-plan.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def).toEqual({
			name: "mach12:issue-plan",
			filePath: "/abs/mach12:issue-plan.md",
			body: "Body text.",
			description: "A demo command.",
			allowedTools: ["Read", "Bash"],
			next: { mode: "ask", hint: "User picks next" },
		});
	});

	it("leaves optional fields unset for a bare file", () => {
		const result = parseCommandFile("/abs/mach12:bare.md", "---\n---\nBody.", SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
		expect(result.def.allowedTools).toBeUndefined();
		expect(result.def.next).toBeUndefined();
	});

	it("treats a comma-string allowed-tools as a list", () => {
		const content = "---\nallowed-tools: Read, Bash\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:tools.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.allowedTools).toEqual(["Read", "Bash"]);
	});

	it("strips a leading UTF-8 BOM before parsing frontmatter", () => {
		const content = "﻿---\ndescription: BOM survives\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bom.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBe("BOM survives");
	});

	it("rejects files whose basename does not start with the set prefix", () => {
		const result = parseCommandFile("/abs/issue-plan.md", "---\n---\nBody.", SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("mach12:");
	});

	it("rejects files that are not markdown", () => {
		const result = parseCommandFile("/abs/mach12:issue-plan.txt", "---\n---\nBody.", SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("markdown");
	});

	it("rejects files with a malformed next block", () => {
		const content = "---\nnext:\n  mode: bogus\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bad.md", content, SET);
		expect(result.ok).toBe(false);
		if (result.ok) return;
		expect(result.error).toContain("bogus");
	});

	it("ignores an empty-string description", () => {
		const content = "---\ndescription: ''\n---\nBody.";
		const result = parseCommandFile("/abs/mach12:bare.md", content, SET);
		expect(result.ok).toBe(true);
		if (!result.ok) return;
		expect(result.def.description).toBeUndefined();
	});
});

describe("buildRegistry — collision and skip semantics", () => {
	function entry(setName: string, file: string, content: string, scope: "global" | "project" = "global"): FileEntry {
		return { filePath: `/fake/${setName}/${file}`, content, setName, scope };
	}

	const minimal = "---\n---\nBody.";

	it("returns an empty registry and no warnings for empty input", () => {
		const out = buildRegistry([]);
		expect(out.registry.size).toBe(0);
		expect(out.warnings).toEqual([]);
	});

	it("registers a single valid entry", () => {
		const out = buildRegistry([entry("mach12", "mach12:issue-plan.md", minimal)]);
		expect(out.registry.size).toBe(1);
		expect(out.registry.has("mach12:issue-plan")).toBe(true);
		expect(out.warnings).toEqual([]);
	});

	it("registers multiple non-colliding entries", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:a.md", minimal),
			entry("mach12", "mach12:b.md", minimal),
			entry("infra", "infra:rotate.md", minimal),
		]);
		expect(out.registry.size).toBe(3);
	});

	it("global wins when project-local name collides", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:issue-plan.md", minimal, "global"),
			entry("mach12", "mach12:issue-plan.md", minimal, "project"),
		]);
		expect(out.registry.size).toBe(1);
		const def = out.registry.get("mach12:issue-plan");
		expect(def?.filePath).toBe("/fake/mach12/mach12:issue-plan.md");
		expect(out.warnings).toHaveLength(1);
		expect(out.warnings[0]).toContain("project");
		expect(out.warnings[0]).toContain("mach12:issue-plan");
	});

	it("logs and skips malformed files but keeps valid ones in the same batch", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:good.md", minimal),
			entry("mach12", "issue-plan.md", minimal),
			entry("mach12", "mach12:bad.md", "---\nnext:\n  mode: bogus\n---\nBody."),
			entry("mach12", "mach12:also-good.md", minimal),
		]);
		expect(out.registry.size).toBe(2);
		expect(out.registry.has("mach12:good")).toBe(true);
		expect(out.registry.has("mach12:also-good")).toBe(true);
		expect(out.warnings).toHaveLength(2);
	});

	it("preserves input order so later globals win over earlier projects (defensive)", () => {
		const out = buildRegistry([
			entry("mach12", "mach12:x.md", minimal, "global"),
			entry("mach12", "mach12:x.md", minimal, "project"),
			entry("mach12", "mach12:x.md", minimal, "global"),
		]);
		expect(out.registry.size).toBe(1);
		expect(out.warnings).toHaveLength(2);
	});
});

describe("registerCommandLoader — fixture-backed integration", () => {
	let originalCache: string | undefined;
	let warnSpy: ReturnType<typeof vi.spyOn>;

	beforeEach(() => {
		originalCache = process.env.SCRAMJET_CACHE;
		warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
	});

	afterEach(() => {
		if (originalCache === undefined) delete process.env.SCRAMJET_CACHE;
		else process.env.SCRAMJET_CACHE = originalCache;
		warnSpy.mockRestore();
	});

	it("registers exactly one resources_discover handler", () => {
		const { pi, handlers } = recordingPi();
		registerCommandLoader(pi, freshState());
		expect(handlers.has("resources_discover")).toBe(true);
		expect(handlers.size).toBe(1);
	});

	it("populates state.registry from global + project fixtures", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		expect(handler).toBeDefined();
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		}) as { promptPaths: string[] };

		expect(state.registry.has("mach12:issue-plan")).toBe(true);
		expect(state.registry.has("mach12:pr-review")).toBe(true);
		expect(state.registry.has("infra:rotate-key")).toBe(true);
		expect(state.registry.has("local:notes")).toBe(true);
		expect(state.registry.has("mach12:broken")).toBe(false);

		const globalIssuePlan = state.registry.get("mach12:issue-plan");
		expect(globalIssuePlan?.filePath).toContain("loader-global");
		expect(globalIssuePlan?.next).toEqual({
			mode: "open",
			candidates: [
				{ name: "mach12:issue-review", hint: "Use when the plan is non-trivial or touches risky areas." },
				{ name: "mach12:issue-implement", hint: "Use when the plan is small and uncontroversial." },
			],
		});
		expect(globalIssuePlan?.allowedTools).toEqual(["Read", "Bash"]);

		expect(result.promptPaths).toEqual([...state.registry.values()].map((d) => d.filePath));
		for (const p of result.promptPaths) {
			expect(p.endsWith(".md")).toBe(true);
		}

		const warnings = warnSpy.mock.calls.map((c) => String(c[0]));
		expect(warnings.some((m) => m.includes("issue-plan.md") && m.includes("mach12:"))).toBe(true);
		expect(warnings.some((m) => m.includes("mach12:broken"))).toBe(true);
		expect(warnings.some((m) => m.includes("project") && m.includes("mach12:issue-plan"))).toBe(true);
	});

	it("rebuilds the registry on each handler invocation", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "startup" });
		const firstSize = state.registry.size;
		state.registry.set("ghost:command", {
			name: "ghost:command",
			filePath: "/nope",
			body: "",
		});
		handler?.({ type: "resources_discover", cwd: join(FIXTURES, "loader-project"), reason: "reload" });
		expect(state.registry.size).toBe(firstSize);
		expect(state.registry.has("ghost:command")).toBe(false);
	});

	it("handles missing global root gracefully", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "does-not-exist");
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "loader-project"),
			reason: "startup",
		}) as { promptPaths: string[] };
		expect(state.registry.has("local:notes")).toBe(true);
		expect(result.promptPaths.length).toBeGreaterThan(0);
	});

	it("handles missing project root gracefully", () => {
		process.env.SCRAMJET_CACHE = join(FIXTURES, "loader-global");
		const { pi, handlers } = recordingPi();
		const state = freshState();
		registerCommandLoader(pi, state);
		const handler = handlers.get("resources_discover")![0];
		const result = handler?.({
			type: "resources_discover",
			cwd: join(FIXTURES, "does-not-exist"),
			reason: "startup",
		}) as { promptPaths: string[] };
		expect(state.registry.has("mach12:issue-plan")).toBe(true);
		expect(result.promptPaths.length).toBeGreaterThan(0);
	});
});
