import { describe, expect, it, vi } from "vitest";
import { registerClearAlias } from "../clear-alias.ts";

// Minimal Pi mock: capture registerCommand calls so we can inspect the
// command name, description, and exercise the handler. The handler's only
// observable effect is calling ctx.newSession(); a regression where /clear
// is registered but ctx.newSession is no longer invoked would silently
// break Claude Code muscle memory.
function recordingPi() {
	const registered: Array<{ name: string; spec: { description?: string; handler: (...args: unknown[]) => unknown } }> =
		[];
	const pi: any = {
		registerCommand(name: string, spec: { description?: string; handler: (...args: unknown[]) => unknown }) {
			registered.push({ name, spec });
		},
	};
	return { pi, registered };
}

describe("registerClearAlias", () => {
	it("registers exactly one command", () => {
		const { pi, registered } = recordingPi();
		registerClearAlias(pi);
		expect(registered).toHaveLength(1);
	});

	it("registers under the name `clear`", () => {
		const { pi, registered } = recordingPi();
		registerClearAlias(pi);
		expect(registered[0].name).toBe("clear");
	});

	it("supplies a non-empty description", () => {
		const { pi, registered } = recordingPi();
		registerClearAlias(pi);
		expect(registered[0].spec.description).toBeTruthy();
		expect(typeof registered[0].spec.description).toBe("string");
	});

	it("handler calls ctx.newSession() exactly once", async () => {
		const { pi, registered } = recordingPi();
		registerClearAlias(pi);
		const newSession = vi.fn(async () => {});
		const ctx = { newSession };
		await registered[0].spec.handler(undefined, ctx);
		expect(newSession).toHaveBeenCalledTimes(1);
	});
});
