import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createLogger, SCRAMJET_LOG_TYPE } from "../logger.ts";

function fakePi() {
	const appended: { customType: string; data: unknown }[] = [];
	return {
		pi: { appendEntry: (customType: string, data: unknown) => appended.push({ customType, data }) },
		appended,
	};
}

describe("createLogger", () => {
	let stderrSpy: { mockRestore(): void; mock: { calls: unknown[][] } };

	beforeEach(() => {
		stderrSpy = vi.spyOn(process.stderr, "write").mockImplementation(() => true);
	});
	afterEach(() => {
		stderrSpy.mockRestore();
	});

	it("warn() appends a scramjet:log entry with correct shape", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.warn("discovery", "something failed");
		expect(appended).toHaveLength(1);
		expect(appended[0].customType).toBe(SCRAMJET_LOG_TYPE);
		const data = appended[0].data as any;
		expect(data.level).toBe("warn");
		expect(data.category).toBe("discovery");
		expect(data.message).toBe("something failed");
		expect(typeof data.timestamp).toBe("number");
	});

	it("lifecycle() appends entry with structured { from, to, event, command } data", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.lifecycle("phase transition", {
			from: "idle",
			to: "running",
			event: "command_start",
			command: "mach12:push",
		});
		expect(appended).toHaveLength(1);
		const data = appended[0].data as any;
		expect(data.level).toBe("lifecycle");
		expect(data.category).toBe("lifecycle");
		expect(data.message).toBe("phase transition");
		expect(data.data).toEqual({ from: "idle", to: "running", event: "command_start", command: "mach12:push" });
	});

	it("debug() appends entry with level debug", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.debug("discovery", "bridge info");
		expect(appended).toHaveLength(1);
		const data = appended[0].data as any;
		expect(data.level).toBe("debug");
		expect(data.category).toBe("discovery");
		expect(data.message).toBe("bridge info");
	});

	it("warn() writes stderr when hasUI is false", () => {
		const { pi } = fakePi();
		const logger = createLogger(pi as any);
		logger.warn("scope", "out of scope");
		expect(stderrSpy).toHaveBeenCalledOnce();
		const written = stderrSpy.mock.calls[0][0];
		expect(written).toContain("out of scope");
	});

	it("warn() does NOT write stderr when hasUI is true", () => {
		const { pi } = fakePi();
		const logger = createLogger(pi as any);
		logger.setHasUI(true);
		logger.warn("scope", "out of scope");
		expect(stderrSpy).not.toHaveBeenCalled();
	});

	it("debug() never writes stderr regardless of hasUI", () => {
		const { pi } = fakePi();
		const logger = createLogger(pi as any);
		logger.debug("discovery", "verbose info");
		expect(stderrSpy).not.toHaveBeenCalled();
		logger.setHasUI(false);
		logger.debug("discovery", "still verbose");
		expect(stderrSpy).not.toHaveBeenCalled();
	});

	it("lifecycle() never writes stderr regardless of hasUI", () => {
		const { pi } = fakePi();
		const logger = createLogger(pi as any);
		logger.lifecycle("transition", { from: "idle", to: "running" });
		expect(stderrSpy).not.toHaveBeenCalled();
		logger.setHasUI(false);
		logger.lifecycle("transition", { from: "running", to: "probing" });
		expect(stderrSpy).not.toHaveBeenCalled();
	});

	it("data field omitted from entry when not provided to warn()", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.warn("scope", "no extra data");
		const data = appended[0].data as any;
		expect(data).not.toHaveProperty("data");
	});

	it("data field included when provided to warn()", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.warn("scope", "with data", { tool: "write" });
		const data = appended[0].data as any;
		expect(data.data).toEqual({ tool: "write" });
	});

	it("data field omitted from entry when not provided to debug()", () => {
		const { pi, appended } = fakePi();
		const logger = createLogger(pi as any);
		logger.debug("discovery", "no extra data");
		const data = appended[0].data as any;
		expect(data).not.toHaveProperty("data");
	});

	it("swallows appendEntry failures without propagating", () => {
		const pi = {
			appendEntry() {
				throw new Error("disk full");
			},
		};
		const logger = createLogger(pi as any);
		expect(() => logger.warn("scope", "should not throw")).not.toThrow();
		expect(() => logger.debug("discovery", "should not throw")).not.toThrow();
		expect(() => logger.lifecycle("transition", { from: "idle", to: "running" })).not.toThrow();
	});

	it("still writes stderr when appendEntry throws and hasUI is false", () => {
		const pi = {
			appendEntry() {
				throw new Error("disk full");
			},
		};
		const logger = createLogger(pi as any);
		logger.warn("scope", "visible warning");
		expect(stderrSpy).toHaveBeenCalledOnce();
		const written = stderrSpy.mock.calls[0][0];
		expect(written).toContain("visible warning");
	});
});
