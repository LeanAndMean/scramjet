import type { ExecOptions, ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import {
	ForgeCommandError,
	type ForgeExec,
	parseForgeRemote,
	resolveCurrentRepository,
	runForgeCommand,
	withForgeMutationQueue,
} from "../src/forge/client.js";

function result(overrides: Partial<ExecResult> = {}): ExecResult {
	return { stdout: "", stderr: "", code: 0, killed: false, ...overrides };
}

describe("parseForgeRemote", () => {
	it.each([
		["https://github.com/LeanAndMean/scramjet.git", "github", "LeanAndMean/scramjet"],
		["git@github.com:LeanAndMean/scramjet.git", "github", "LeanAndMean/scramjet"],
		["ssh://git@github.com/LeanAndMean/scramjet.git", "github", "LeanAndMean/scramjet"],
		["https://gitlab.com/group/subgroup/widget.git", "gitlab", "group/subgroup/widget"],
		["git@gitlab.com:group/subgroup/widget.git", "gitlab", "group/subgroup/widget"],
		["ssh://git@gitlab.com/group/subgroup/widget.git", "gitlab", "group/subgroup/widget"],
	] as const)("parses %s", (remote, forge, projectPath) => {
		expect(parseForgeRemote(remote)).toEqual({
			forge,
			host: `${forge}.com`,
			projectPath,
		});
	});

	it.each([
		"http://github.com/owner/repo",
		"https://user:secret@github.com/owner/repo.git",
		"https://github.com:443/owner/repo.git",
		"https://github.com/owner/repo.git?token=x",
		"https://github.com/owner/repo.git#fragment",
		"https://github.com/owner/repo.git?",
		"https:github.com/owner/repo.git",
		"https://github.com/owner/tmp/../repo.git",
		"https://github.com/owner/%2e%2e/repo.git",
		"https://github.com\\owner\\repo.git",
		"https://%67ithub.com/owner/repo.git",
		"https://@github.com/owner/repo.git",
		"https://github.example.com/owner/repo.git",
		"git@github-work:owner/repo.git",
		"git@github.com:owner/extra/repo.git",
		"https://github.com/owner",
		"https://gitlab.com/group",
		"https://gitlab.com/group//repo",
		"ssh://other@github.com/owner/repo.git",
		"file:///tmp/repo",
		"not a remote",
	] as const)("rejects unsupported or malformed remote %s", (remote) => {
		expect(() => parseForgeRemote(remote)).toThrow(/supported GitHub or GitLab origin/);
	});
});

describe("resolveCurrentRepository", () => {
	it("reads only origin and forwards cwd and cancellation", async () => {
		const controller = new AbortController();
		const exec = vi.fn<ForgeExec>(async () => result({ stdout: "https://github.com/LeanAndMean/scramjet.git\n" }));

		await expect(resolveCurrentRepository(exec, "/worktree", controller.signal)).resolves.toEqual({
			forge: "github",
			host: "github.com",
			projectPath: "LeanAndMean/scramjet",
		});
		expect(exec).toHaveBeenCalledWith("git", ["remote", "get-url", "origin"], {
			cwd: "/worktree",
			signal: controller.signal,
			timeout: 3000,
		});
	});

	it("surfaces a missing origin without attempting another command", async () => {
		const exec = vi.fn<ForgeExec>(async () => result({ code: 2, stderr: "No such remote" }));
		await expect(resolveCurrentRepository(exec, "/repo")).rejects.toMatchObject({
			name: "ForgeCommandError",
			kind: "failed",
		});
		expect(exec).toHaveBeenCalledTimes(1);
	});
});

describe("runForgeCommand", () => {
	const invocation = { command: "gh", args: ["api", "repos/acme/widget/issues/7"], cwd: "/repo" };

	it.each([
		[result({ spawnError: { message: "spawn gh ENOENT", code: "ENOENT" } }), "missing-executable"],
		[result({ killed: true }), "timeout"],
		[result({ stdinError: { message: "write EPIPE", code: "EPIPE" } }), "stdin"],
		[result({ code: 1, stderr: "failed" }), "failed"],
	] as const)("classifies process failure as %s", async (reply, kind) => {
		const exec: ForgeExec = async () => reply;
		await expect(runForgeCommand(exec, invocation)).rejects.toMatchObject({ name: "ForgeCommandError", kind });
	});

	it("distinguishes cancellation from timeout", async () => {
		const controller = new AbortController();
		controller.abort();
		const exec: ForgeExec = async () => result({ killed: true });
		await expect(runForgeCommand(exec, { ...invocation, signal: controller.signal })).rejects.toMatchObject({
			kind: "cancelled",
		});
	});

	it("returns successful process output and exact options", async () => {
		const exec = vi.fn<ForgeExec>(async () => result({ stdout: "{}\n" }));
		await expect(
			runForgeCommand(exec, { ...invocation, stdin: '{"body":"hello"}', timeout: 1234 }),
		).resolves.toMatchObject({ stdout: "{}\n" });
		expect(exec).toHaveBeenCalledWith(invocation.command, invocation.args, {
			cwd: "/repo",
			stdin: '{"body":"hello"}',
			timeout: 1234,
			signal: undefined,
		});
	});

	it("retains bounded safe diagnostics and shared authentication guidance without exposing stdin content", async () => {
		const body = "<private mutation body>";
		const stdin = JSON.stringify({ title: "private title", body });
		const exec: ForgeExec = async () =>
			result({
				code: 1,
				stdout: '{"body":"\\u003cprivate mutation body\\u003e","title":"private title"}\u001b[31m',
				stderr: `HTTP 401 Unauthorized ${"x".repeat(10_000)}`,
			});
		let caught: unknown;
		try {
			await runForgeCommand(exec, { ...invocation, stdin });
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(ForgeCommandError);
		const error = caught as ForgeCommandError;
		expect(error.authenticationFailure).toBe(true);
		expect(error.message).toContain("exit code 1");
		expect(error.message).toContain("[redacted]");
		expect(error.message).toContain("\\u001b");
		expect(error.message).toContain("gh auth login --hostname github.com");
		expect(error.message).not.toContain(stdin);
		expect(error.message).not.toContain(body);
		expect(error.message).not.toContain("private title");
		expect(error.invocation.process?.stderr).toContain("[truncated]");
		expect(Buffer.byteLength(error.invocation.process?.stderr ?? "", "utf8")).toBeLessThan(4200);
	});

	it("does not classify authentication from echoed mutation content", async () => {
		const stdin = JSON.stringify({ body: "HTTP 401 Unauthorized" });
		const exec: ForgeExec = async () =>
			result({ code: 1, stderr: `validation rejected ${JSON.stringify("HTTP 401 Unauthorized")}` });
		let caught: unknown;
		try {
			await runForgeCommand(exec, { ...invocation, stdin });
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(ForgeCommandError);
		const error = caught as ForgeCommandError;
		expect(error.authenticationFailure).toBe(false);
		expect(error.message).not.toContain("auth login");
		expect(error.message).not.toContain("HTTP 401 Unauthorized");
	});

	it("reports stdin only by byte count and digest", async () => {
		const secret = "private body λ";
		const exec: ForgeExec = async () => result({ code: 1 });
		let caught: unknown;
		try {
			await runForgeCommand(exec, { ...invocation, stdin: secret });
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(ForgeCommandError);
		const error = caught as ForgeCommandError;
		expect(error.message).not.toContain(secret);
		expect(error.invocation.stdin).toEqual({
			bytes: Buffer.byteLength(secret),
			sha256: "f4fb0bee2ea17e63a8c40a8210db8d1397d7efd00fc2f25fe9eeb12a0f75ae2c",
		});
	});
});

describe("withForgeMutationQueue", () => {
	it("serializes the same key in call order", async () => {
		const events: string[] = [];
		let release!: () => void;
		const barrier = new Promise<void>((resolve) => {
			release = resolve;
		});
		const first = withForgeMutationQueue("github:acme/widget:issue:7", async () => {
			events.push("first:start");
			await barrier;
			events.push("first:end");
		});
		const second = withForgeMutationQueue("github:acme/widget:issue:7", async () => {
			events.push("second:start");
		});

		await Promise.resolve();
		expect(events).toEqual(["first:start"]);
		release();
		await Promise.all([first, second]);
		expect(events).toEqual(["first:start", "first:end", "second:start"]);
	});

	it("permits different keys to proceed concurrently", async () => {
		const events: string[] = [];
		let release!: () => void;
		const barrier = new Promise<void>((resolve) => {
			release = resolve;
		});
		const first = withForgeMutationQueue("issue:1", async () => {
			events.push("first");
			await barrier;
		});
		const second = withForgeMutationQueue("issue:2", async () => {
			events.push("second");
		});

		await second;
		expect(events).toEqual(["first", "second"]);
		release();
		await first;
	});

	it("releases the key after a rejected operation", async () => {
		await expect(
			withForgeMutationQueue("issue:3", async () => {
				throw new Error("boom");
			}),
		).rejects.toThrow("boom");
		await expect(withForgeMutationQueue("issue:3", async () => "next")).resolves.toBe("next");
	});
});

const _execOptionsContract: ExecOptions = { stdin: "body", cwd: "/repo" };
void _execOptionsContract;
