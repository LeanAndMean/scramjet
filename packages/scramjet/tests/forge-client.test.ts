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

	it.each([
		["spawn", result({ spawnError: { message: "permission \u202Edenied", code: "EACCES" } }), "spawnError", "EACCES"],
		["stdin", result({ stdinError: { message: "write \u009Bclosed", code: "EPIPE" } }), "stdinError", "EPIPE"],
	] as const)("retains bounded control-safe %s causes", async (_name, reply, field, code) => {
		let caught: unknown;
		try {
			await runForgeCommand(async () => reply, { ...invocation, stdin: "private body" });
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(ForgeCommandError);
		const process = (caught as ForgeCommandError).invocation.process;
		expect(process?.[field]).toMatchObject({ code });
		expect(process?.[field]?.message).not.toMatch(/[\u009B\u202E]/u);
		expect(process?.[field]?.message).toContain("\\u");
		expect(JSON.stringify(process)).not.toContain("private body");
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
		expect(error.invocation.process?.authenticationFailure).toBe(true);
		expect(error.message).toContain("exit code 1");
		expect(error.message).toContain("[suppressed 47 bytes; sha256");
		expect(error.message).toContain("gh auth login --hostname github.com");
		expect(error.message).not.toContain(stdin);
		expect(error.message).not.toContain(body);
		expect(error.message).not.toContain("private title");
		expect(error.invocation.process?.stdout).toMatch(/^\[suppressed \d+ bytes; sha256 [a-f0-9]{64}\]$/);
		expect(error.invocation.process?.stderr).toMatch(/^\[suppressed \d+ bytes; sha256 [a-f0-9]{64}\]$/);
		expect(JSON.stringify(error.invocation)).not.toContain("private mutation body");
	});

	it("fingerprints GraphQL query arguments in exposed diagnostics", async () => {
		const query = `query VeryLarge { ${"field ".repeat(1000)} }`;
		let caught: unknown;
		try {
			await runForgeCommand(async () => result({ code: 1, stderr: "failed" }), {
				...invocation,
				args: ["api", "graphql", "-f", `query=${query}`],
			});
		} catch (error) {
			caught = error;
		}
		expect(caught).toBeInstanceOf(ForgeCommandError);
		const message = (caught as ForgeCommandError).message;
		expect(message).toContain("query=<sha256:");
		expect(message).not.toContain("query VeryLarge");
	});

	it("escapes terminal control characters in exposed diagnostics", async () => {
		const exec: ForgeExec = async () => result({ code: 1, stderr: "remote rejected \u202Espoof\u009B31m" });
		let caught: unknown;
		try {
			await runForgeCommand(exec, invocation);
		} catch (error) {
			caught = error;
		}

		expect(caught).toBeInstanceOf(ForgeCommandError);
		const message = (caught as ForgeCommandError).message;
		expect(/[\u009B\u202E]/u.test(message)).toBe(false);
		expect(message).toContain(String.raw`\u202Espoof\u009B31m`);
	});

	it.each([
		["complete", "HTTP 401 Unauthorized"],
		["partial", "heading\nHTTP 401 Unauthorized\nfooter"],
	])("does not classify authentication from %s echoed mutation content", async (_name, body) => {
		const stdin = JSON.stringify({ body });
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
		expect(error.invocation.process?.authenticationFailure).toBe(false);
		expect(error.message).not.toContain("auth login");
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
