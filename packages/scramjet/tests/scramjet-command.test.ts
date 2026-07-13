import { describe, expect, it } from "vitest";
import { registerScramjetCommand } from "../src/scramjet-command.js";
import { freshState } from "./helpers.js";

interface RegisteredCommand {
	name: string;
	spec: {
		description?: string;
		getArgumentCompletions?: (prefix: string) => Array<{ value: string; label: string }> | null;
		handler: (args: string, ctx: unknown) => Promise<void> | void;
	};
}

function recordingPi() {
	const commands: RegisteredCommand[] = [];
	const pi: any = {
		registerCommand(name: string, spec: RegisteredCommand["spec"]) {
			commands.push({ name, spec });
		},
	};
	return { pi, commands };
}

function fakeCtx(hasUI = true) {
	const notifications: { message: string; type?: string }[] = [];
	const ctx: any = {
		hasUI,
		ui: {
			notify(message: string, type?: string) {
				notifications.push({ message, type });
			},
		},
	};
	return { ctx, notifications };
}

function spec(commands: RegisteredCommand[]) {
	const entry = commands.find((c) => c.name === "scramjet");
	if (!entry) throw new Error("scramjet command not registered");
	return entry.spec;
}

describe("registerScramjetCommand — registration surface", () => {
	it("registers exactly one command, named 'scramjet', with a description", () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		expect(commands).toHaveLength(1);
		expect(commands[0].name).toBe("scramjet");
		expect(commands[0].spec.description).toBeTruthy();
	});

	it("argument completions: 's' offers settings; 'x' returns null", () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const fn = spec(commands).getArgumentCompletions;
		expect(fn).toBeDefined();

		const sResults = fn?.("s");
		expect(sResults?.map((c) => c.value)).toEqual(["settings"]);

		expect(fn?.("x")).toBeNull();
	});
});

describe("registerScramjetCommand — handler", () => {
	it("bare invocation without TUI notifies error", async () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const { ctx, notifications } = fakeCtx(false);

		await spec(commands).handler("", ctx);

		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("error");
		expect(notifications[0].message).toContain("TUI");
	});

	it("'settings' without TUI notifies error", async () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const { ctx, notifications } = fakeCtx(false);

		await spec(commands).handler("settings", ctx);

		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("error");
		expect(notifications[0].message).toContain("TUI");
	});

	it("unknown args produce a usage warning", async () => {
		const { pi, commands } = recordingPi();
		registerScramjetCommand(pi, freshState());
		const { ctx, notifications } = fakeCtx();

		await spec(commands).handler("foobar", ctx);

		expect(notifications).toHaveLength(1);
		expect(notifications[0].type).toBe("warning");
		expect(notifications[0].message).toContain("Usage:");
	});
});
