import { getEventListeners, setMaxListeners } from "node:events";
import { resolve } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { createLsTool, type LsOperations } from "../src/core/tools/ls.js";
import { DEFAULT_MAX_BYTES } from "../src/core/tools/truncate.js";

const cwd = process.cwd();
const directory = { isDirectory: () => true };
const file = { isDirectory: () => false };

function operations(overrides: Partial<LsOperations> = {}): LsOperations {
	return { exists: () => true, stat: () => directory, readdir: () => [], ...overrides };
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => {
		resolve = settle;
	});
	return { promise, resolve };
}

describe("ls abort-listener lifetime", () => {
	it.each([
		["missing path", operations({ exists: () => false }), `Path not found: ${resolve(cwd, "target")}`],
		["non-directory", operations({ stat: () => file }), `Not a directory: ${resolve(cwd, "target")}`],
		[
			"directory read failure",
			operations({
				readdir: () => {
					throw new Error("read denied");
				},
			}),
			"Cannot read directory: read denied",
		],
		[
			"stat failure",
			operations({
				stat: () => {
					throw new Error("stat denied");
				},
			}),
			"stat denied",
		],
	] as const)("restores baseline immediately after repeated %s rejection", async (_name, ops, message) => {
		const caller = new AbortController();
		setMaxListeners(10, caller.signal);
		const unrelated = () => {};
		caller.signal.addEventListener("abort", unrelated);
		const baseline = getEventListeners(caller.signal, "abort");
		const tool = createLsTool(cwd, { operations: ops });
		try {
			for (let i = 0; i < 12; i++) {
				await expect(tool.execute(`ls-${i}`, { path: "target" }, caller.signal)).rejects.toThrow(message);
				expect(getEventListeners(caller.signal, "abort")).toEqual(baseline);
				expect(caller.signal.aborted).toBe(false);
			}
		} finally {
			caller.abort();
		}
	});

	it("restores baseline after repeated success and preserves sorted entries and directory suffixes", async () => {
		const caller = new AbortController();
		const tool = createLsTool(cwd, {
			operations: operations({
				readdir: () => ["z.txt", "Folder", ".hidden", "unreadable"],
				stat: (path) => {
					if (path.endsWith("unreadable")) throw new Error("stat denied");
					return path === cwd || path.endsWith("Folder") ? directory : file;
				},
			}),
		});
		for (let i = 0; i < 12; i++) {
			const result = await tool.execute(`ls-${i}`, {}, caller.signal);
			expect(result).toEqual({ content: [{ type: "text", text: ".hidden\nFolder/\nz.txt" }], details: undefined });
			expect(getEventListeners(caller.signal, "abort")).toEqual([]);
		}
		expect(caller.signal.aborted).toBe(false);
	});

	it("preserves empty-directory output, entry limits, and byte truncation details", async () => {
		const caller = new AbortController();
		const run = (ops: LsOperations, limit?: number) =>
			createLsTool(cwd, { operations: ops }).execute("ls", { limit }, caller.signal);
		expect(await run(operations())).toEqual({
			content: [{ type: "text", text: "(empty directory)" }],
			details: undefined,
		});
		const limited = await run(operations({ readdir: () => ["b", "a"] }), 1);
		expect(limited).toEqual({
			content: [{ type: "text", text: "a/\n\n[1 entries limit reached. Use limit=2 for more]" }],
			details: { entryLimitReached: 1 },
		});
		const truncated = await run(operations({ readdir: () => ["x".repeat(DEFAULT_MAX_BYTES + 1)] }));
		expect(truncated.details?.truncation?.truncated).toBe(true);
		expect(truncated.content[0]).toMatchObject({ type: "text", text: expect.stringContaining("limit reached]") });
		expect(getEventListeners(caller.signal, "abort")).toEqual([]);
	});

	it("rejects pre-aborted calls without starting custom operations", async () => {
		const caller = new AbortController();
		caller.abort();
		const exists = vi.fn(() => true);
		const tool = createLsTool(cwd, { operations: operations({ exists }) });
		await expect(tool.execute("ls", {}, caller.signal)).rejects.toThrow("Operation aborted");
		expect(exists).not.toHaveBeenCalled();
		expect(getEventListeners(caller.signal, "abort")).toEqual([]);
	});

	it.each(["exists", "stat", "readdir"] as const)(
		"rejects cancellation before deferred %s settles and drains cooperative work",
		async (phase) => {
			const caller = new AbortController();
			const pending = deferred<never>();
			const ready = deferred<void>();
			const drained = deferred<void>();
			const unrelated = () => {};
			caller.signal.addEventListener("abort", unrelated);
			const tool = createLsTool(cwd, {
				operations: operations({
					[phase]: () => {
						ready.resolve();
						return pending.promise.finally(() => drained.resolve());
					},
				}),
			});
			const result = tool.execute("ls", {}, caller.signal);
			const rejected = expect(result).rejects.toThrow("Operation aborted");
			await ready.promise;
			expect(getEventListeners(caller.signal, "abort")).toHaveLength(2);
			caller.abort();
			try {
				await rejected;
				expect(getEventListeners(caller.signal, "abort")).toEqual([unrelated]);
			} finally {
				pending.resolve((phase === "exists" ? false : phase === "stat" ? file : []) as never);
				await drained.promise;
				await result.catch(() => {});
			}
		},
	);
});
