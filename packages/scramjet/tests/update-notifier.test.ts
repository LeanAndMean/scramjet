import { afterEach, describe, expect, it, vi } from "vitest";
import { registerUpdateNotifier, UPDATE_CHECK_TIMEOUT_MS } from "../src/update-notifier.js";
import { recordingPi } from "./helpers.js";

type ExecResult = { stdout: string; stderr: string; code: number; killed: boolean };

const success = (version: unknown): ExecResult => ({
	stdout: JSON.stringify(version),
	stderr: "",
	code: 0,
	killed: false,
});

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function setup(
	options: { result?: ExecResult | Promise<ExecResult>; managed?: boolean; installedVersion?: string } = {},
) {
	const bag = recordingPi();
	bag.pi.exec = vi.fn(() => Promise.resolve(options.result ?? success("0.71.0")));
	const notify = vi.fn();
	registerUpdateNotifier(bag.pi, {
		installedVersion: () => options.installedVersion ?? "0.70.0",
		isManagedInstallation: () => options.managed ?? true,
	});
	return { bag, notify, ctx: { hasUI: true, ui: { notify } } };
}

afterEach(() => {
	delete process.env.SCRAMJET_OFFLINE;
	delete process.env.PI_OFFLINE;
});

describe("registerUpdateNotifier", () => {
	it("detaches the lookup and invokes npm with bounded exact arguments", async () => {
		let resolveExec!: (result: ExecResult) => void;
		const unresolved = new Promise<ExecResult>((resolve) => {
			resolveExec = resolve;
		});
		const { bag, ctx } = setup({ result: unresolved });

		await bag.emit("session_start", {}, ctx);
		expect(bag.pi.exec).toHaveBeenCalledWith("npm", ["view", "@leanandmean/scramjet", "version", "--json"], {
			timeout: UPDATE_CHECK_TIMEOUT_MS,
		});

		resolveExec(success("0.71.0"));
		await flush();
	});

	it.each([
		{ managed: true, guidance: "scramjet update" },
		{ managed: false, guidance: "Pull the latest source and reinstall" },
	])("notifies for a newer $guidance installation", async ({ managed, guidance }) => {
		const { bag, notify, ctx } = setup({ managed });
		await bag.emit("session_start", {}, ctx);
		await flush();

		expect(notify).toHaveBeenCalledOnce();
		expect(notify).toHaveBeenCalledWith(expect.stringContaining("0.71.0"), "info");
		expect(notify.mock.calls[0][0]).toContain(guidance);
	});

	it.each([
		["equal", success("0.70.0")],
		["older", success("0.69.0")],
		["malformed version", success("latest")],
		["array output", success(["0.71.0"])],
		["object output", success({ version: "0.71.0" })],
		["malformed JSON", { ...success("0.71.0"), stdout: "{" }],
		["empty output", { ...success("0.71.0"), stdout: "" }],
		["nonzero exit", { ...success("0.71.0"), code: 1 }],
		["killed process", { ...success("0.71.0"), killed: true }],
	] as const)("stays silent for %s", async (_label, result) => {
		const { bag, notify, ctx } = setup({ result });
		await bag.emit("session_start", {}, ctx);
		await flush();
		expect(notify).not.toHaveBeenCalled();
	});

	it("stays silent for malformed installed versions and rejected execution", async () => {
		const malformed = setup({ installedVersion: "unknown" });
		await malformed.bag.emit("session_start", {}, malformed.ctx);
		await flush();
		expect(malformed.notify).not.toHaveBeenCalled();

		const rejected = setup({ result: Promise.reject(new Error("unavailable")) });
		await rejected.bag.emit("session_start", {}, rejected.ctx);
		await flush();
		expect(rejected.notify).not.toHaveBeenCalled();
	});

	it.each(["SCRAMJET_OFFLINE", "PI_OFFLINE"])("does not spawn when %s is set", async (name) => {
		process.env[name] = "1";
		const { bag, notify, ctx } = setup();
		await bag.emit("session_start", {}, ctx);
		expect(bag.pi.exec).not.toHaveBeenCalled();
		expect(notify).not.toHaveBeenCalled();
	});

	it("does not consume eligibility in headless mode", async () => {
		const { bag, notify, ctx } = setup();
		await bag.emit("session_start", {}, { ...ctx, hasUI: false });
		expect(bag.pi.exec).not.toHaveBeenCalled();

		await bag.emit("session_start", {}, ctx);
		await flush();
		expect(bag.pi.exec).toHaveBeenCalledOnce();
		expect(notify).toHaveBeenCalledOnce();
	});

	it("runs at most once across repeated eligible session starts", async () => {
		const { bag, notify, ctx } = setup();
		await Promise.all([
			bag.emit("session_start", {}, ctx),
			bag.emit("session_start", {}, ctx),
			bag.emit("session_start", {}, ctx),
		]);
		await flush();
		await bag.emit("session_start", {}, ctx);
		expect(bag.pi.exec).toHaveBeenCalledOnce();
		expect(notify).toHaveBeenCalledOnce();
	});

	it("absorbs install classification and stale-context notification failures", async () => {
		const classificationBag = recordingPi();
		classificationBag.pi.exec = vi.fn(async () => success("0.71.0"));
		const classificationNotify = vi.fn();
		registerUpdateNotifier(classificationBag.pi, {
			installedVersion: () => "0.70.0",
			isManagedInstallation: () => {
				throw new Error("classification failed");
			},
		});
		await classificationBag.emit("session_start", {}, { hasUI: true, ui: { notify: classificationNotify } });
		await flush();
		expect(classificationNotify).not.toHaveBeenCalled();

		const stale = setup();
		stale.notify.mockImplementation(() => {
			throw new Error("disposed");
		});
		await expect(stale.bag.emit("session_start", {}, stale.ctx)).resolves.toBeUndefined();
		await flush();
	});
});
