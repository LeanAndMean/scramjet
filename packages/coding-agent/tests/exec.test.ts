import { performance } from "node:perf_hooks";
import { describe, expect, it } from "vitest";
import { execCommand } from "../src/core/exec.js";

describe("execCommand", () => {
	it.skipIf(process.platform === "win32")(
		"escalates to SIGKILL when a timed-out child ignores SIGTERM",
		async () => {
			const startedAt = performance.now();
			const result = await execCommand(
				process.execPath,
				["-e", "process.on('SIGTERM', () => {}); setInterval(() => {}, 1000)"],
				process.cwd(),
				{ timeout: 100 },
			);

			expect(result.killed).toBe(true);
			expect(performance.now() - startedAt).toBeLessThan(6500);
		},
		7000,
	);
});
