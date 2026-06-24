import { describe, expect, it, vi } from "vitest";
import { EXEC_TIMEOUT_MS, type ExecFn, registerPrIndicator, resolvePr } from "../src/pr-indicator.js";
import { recordingPi } from "./helpers.js";

type ExecReply = { stdout: string; code: number };

const GH_REMOTE: ExecReply = { stdout: "https://github.com/LeanAndMean/scramjet.git\n", code: 0 };
const GL_REMOTE: ExecReply = { stdout: "https://gitlab.com/acme/widget.git\n", code: 0 };
const BRANCH: ExecReply = { stdout: "feature/issue-75-show-active-pr\n", code: 0 };
const ONE_PR: ExecReply = { stdout: JSON.stringify([{ number: 72 }]), code: 0 };

// Fake ExecFn keyed by command. Each leg defaults to a "show PR #72" happy path
// so a test overrides only the leg it exercises. `calls` records every
// invocation so tests can assert that, e.g., gh was never spawned.
function fakeExec(opts: { remote?: ExecReply; branch?: ExecReply; prList?: ExecReply; calls?: string[] } = {}): ExecFn {
	return async (cmd, args) => {
		opts.calls?.push([cmd, ...args].join(" "));
		if (cmd === "git" && args[0] === "remote") return opts.remote ?? GH_REMOTE;
		if (cmd === "git" && args[0] === "rev-parse") return opts.branch ?? BRANCH;
		if (cmd === "gh") return opts.prList ?? ONE_PR;
		return { stdout: "", code: 1 };
	};
}

describe("resolvePr", () => {
	it("returns the PR number for a GitHub remote with exactly one open PR", async () => {
		expect(await resolvePr(fakeExec())).toBe(72);
	});

	it("returns null when zero PRs match", async () => {
		expect(await resolvePr(fakeExec({ prList: { stdout: "[]", code: 0 } }))).toBeNull();
	});

	it("returns null when multiple PRs match", async () => {
		const prList = { stdout: JSON.stringify([{ number: 54 }, { number: 52 }, { number: 50 }]), code: 0 };
		expect(await resolvePr(fakeExec({ prList }))).toBeNull();
	});

	it("returns null for a non-GitHub remote and never invokes gh", async () => {
		const calls: string[] = [];
		expect(await resolvePr(fakeExec({ remote: GL_REMOTE, calls }))).toBeNull();
		expect(calls.some((c) => c.startsWith("gh"))).toBe(false);
	});

	it("returns null when `git remote get-url` fails (not a git repo)", async () => {
		expect(await resolvePr(fakeExec({ remote: { stdout: "", code: 1 } }))).toBeNull();
	});

	it("returns null on a detached HEAD", async () => {
		expect(await resolvePr(fakeExec({ branch: { stdout: "HEAD\n", code: 0 } }))).toBeNull();
	});

	it("returns null when the branch read fails", async () => {
		expect(await resolvePr(fakeExec({ branch: { stdout: "", code: 1 } }))).toBeNull();
	});

	it("returns null when gh fails (missing or unauthenticated)", async () => {
		expect(await resolvePr(fakeExec({ prList: { stdout: "", code: 1 } }))).toBeNull();
	});

	it("returns null on malformed JSON", async () => {
		expect(await resolvePr(fakeExec({ prList: { stdout: "not json", code: 0 } }))).toBeNull();
	});

	it("returns null when the matched element has no numeric `number`", async () => {
		expect(await resolvePr(fakeExec({ prList: { stdout: JSON.stringify([{ number: "72" }]), code: 0 } }))).toBeNull();
		expect(await resolvePr(fakeExec({ prList: { stdout: JSON.stringify([{}]), code: 0 } }))).toBeNull();
	});
});

type ExecResult = { stdout: string; stderr: string; code: number; killed: boolean };

// Mutable exec config so a test can flip the branch / PR between session_start
// and agent_end. Returns a vi.fn shaped like pi.exec (full ExecResult). The
// third `options` arg is named (not discarded) so tests can assert that cwd and
// timeout are threaded through into pi.exec.
type ExecOptions = { cwd?: string; timeout?: number };
function execMockFor(cfg: { remote: ExecReply; branch: ExecReply; prList: ExecReply }) {
	const wrap = (r: ExecReply): ExecResult => ({ ...r, stderr: "", killed: false });
	return vi.fn(async (cmd: string, args: string[], _options?: ExecOptions): Promise<ExecResult> => {
		if (cmd === "git" && args[0] === "remote") return wrap(cfg.remote);
		if (cmd === "git" && args[0] === "rev-parse") return wrap(cfg.branch);
		if (cmd === "gh") return wrap(cfg.prList);
		return wrap({ stdout: "", code: 1 });
	});
}

function fakeCtx(setStatus: ReturnType<typeof vi.fn>) {
	return { cwd: "/repo", hasUI: true, ui: { setStatus } };
}

function ghCalled(mock: ReturnType<typeof vi.fn>): boolean {
	return mock.mock.calls.some((c) => c[0] === "gh");
}

describe("registerPrIndicator", () => {
	it("registers handlers on session_start, session_tree, and agent_end only", () => {
		const bag = recordingPi();
		registerPrIndicator(bag.pi);
		expect(bag.handlers.has("session_start")).toBe(true);
		expect(bag.handlers.has("session_tree")).toBe(true);
		expect(bag.handlers.has("agent_end")).toBe(true);
		expect(bag.handlers.has("before_agent_start")).toBe(false);
	});

	it("shows `PR #72` on session_start with one open PR", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		bag.pi.exec = execMockFor(cfg);
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_start", {}, fakeCtx(setStatus));
		expect(setStatus).toHaveBeenCalledWith("scramjet-pr", "PR #72");
	});

	it("clears the indicator on session_start with zero PRs", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: { stdout: "[]", code: 0 } };
		bag.pi.exec = execMockFor(cfg);
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_start", {}, fakeCtx(setStatus));
		expect(setStatus).toHaveBeenCalledWith("scramjet-pr", undefined);
	});

	it("behaves identically on session_tree", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		bag.pi.exec = execMockFor(cfg);
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_tree", {}, fakeCtx(setStatus));
		expect(setStatus).toHaveBeenCalledWith("scramjet-pr", "PR #72");
	});

	it("does no work in headless mode (hasUI false): no spawns, no setStatus", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		const exec = execMockFor(cfg);
		bag.pi.exec = exec;
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		const headlessCtx = { cwd: "/repo", hasUI: false, ui: { setStatus } };
		await bag.emit("session_start", {}, headlessCtx);
		await bag.emit("agent_end", {}, headlessCtx);

		expect(exec).not.toHaveBeenCalled();
		expect(setStatus).not.toHaveBeenCalled();
	});

	it("threads cwd and timeout into every pi.exec call", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		const exec = execMockFor(cfg);
		bag.pi.exec = exec;
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_start", {}, fakeCtx(setStatus));

		// A regression that dropped cwd would silently resolve the wrong repo's PR
		// in a worktree/multi-root session; assert the options arg on every call.
		expect(exec.mock.calls.length).toBeGreaterThan(0);
		for (const call of exec.mock.calls) {
			expect(call[2]).toMatchObject({ cwd: "/repo", timeout: EXEC_TIMEOUT_MS });
		}
	});

	it("skips the gh lookup on agent_end when the branch is unchanged", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		const exec = execMockFor(cfg);
		bag.pi.exec = exec;
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_start", {}, fakeCtx(setStatus));
		exec.mockClear();

		await bag.emit("agent_end", {}, fakeCtx(setStatus));
		expect(ghCalled(exec)).toBe(false);
	});

	it("re-resolves on agent_end when the branch changed", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: BRANCH, prList: ONE_PR };
		const exec = execMockFor(cfg);
		bag.pi.exec = exec;
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await bag.emit("session_start", {}, fakeCtx(setStatus));
		expect(setStatus).toHaveBeenLastCalledWith("scramjet-pr", "PR #72");

		// Agent checked out a different branch with a different open PR.
		cfg.branch = { stdout: "feature/other\n", code: 0 };
		cfg.prList = { stdout: JSON.stringify([{ number: 99 }]), code: 0 };
		await bag.emit("agent_end", {}, fakeCtx(setStatus));

		expect(ghCalled(exec)).toBe(true);
		expect(setStatus).toHaveBeenLastCalledWith("scramjet-pr", "PR #99");
	});

	it("returns early without throwing when git rev-parse fails on agent_end", async () => {
		const bag = recordingPi();
		const cfg = { remote: GH_REMOTE, branch: { stdout: "", code: 1 }, prList: ONE_PR };
		const exec = execMockFor(cfg);
		bag.pi.exec = exec;
		const setStatus = vi.fn();
		registerPrIndicator(bag.pi);

		await expect(bag.emit("agent_end", {}, fakeCtx(setStatus))).resolves.toBeUndefined();
		expect(ghCalled(exec)).toBe(false);
	});
});
