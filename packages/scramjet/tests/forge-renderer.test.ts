import { describe, expect, it } from "vitest";
import { executeForgeReadPlan, windowForgeRead } from "../src/forge/native-reply.js";
import { prettyForgeReply, rawForgeReply } from "../src/forge/renderer.js";
import type { ForgeReadPlan } from "../src/forge/types.js";

const plan: ForgeReadPlan = {
	repository: { forge: "github", host: "github.com", projectPath: "Acme/widget" },
	artifact: { kind: "issue", number: 7 },
	include: ["artifact", "comments"],
	segments: [
		{ id: "artifact", command: "gh", args: ["api", "issue"], shape: { kind: "json" }, evidence: "artifact" },
		{ id: "comments", command: "gh", args: ["api", "comments"], shape: { kind: "gh-slurp" }, evidence: "comments" },
	],
};

describe("forge persisted-reply renderer", () => {
	it("derives GitHub header, Markdown, and comment views only from persisted payload and receipt maps", async () => {
		const outputs = [
			JSON.stringify({
				title: "Parser",
				body: "Body\u202Espoof",
				state: "open",
				html_url: "https://github.com/Acme/widget/issues/7",
			}),
			JSON.stringify([[{ id: 1, body: "Comment\u009Bred", user: { login: "alice" } }]]),
		];
		let index = 0;
		const read = await executeForgeReadPlan(
			plan,
			async () => ({ stdout: outputs[index++], stderr: "", code: 0, killed: false }),
			"/repo",
		);
		const window = windowForgeRead(read, { include: plan.include });
		const pretty = prettyForgeReply(window.content, window.details);
		expect(pretty).toContain("# Parser");
		expect(pretty).toContain("## Comment by alice");
		expect(pretty).toContain("\\u202E");
		expect(pretty).toContain("\\u009B");
		expect(pretty).not.toMatch(/[\u009B\u202E]/u);
	});

	it("renders pinned GitHub commit and GitLab diff shapes without provider-neutral guessing", async () => {
		const githubPlan: ForgeReadPlan = {
			...plan,
			include: ["commits"],
			segments: [{ id: "commits", command: "gh", args: ["api", "commits"], shape: { kind: "gh-slurp" } }],
		};
		const github = await executeForgeReadPlan(
			githubPlan,
			async () => ({
				stdout: JSON.stringify([
					[{ sha: "abcdef1234567890", commit: { message: "First line\nbody", author: { name: "Alice" } } }],
				]),
				stderr: "",
				code: 0,
				killed: false,
			}),
			"/repo",
		);
		const githubWindow = windowForgeRead(github, { include: ["commits"] });
		expect(prettyForgeReply(githubWindow.content, githubWindow.details)).toContain(
			"abcdef123456 | First line | Alice",
		);

		const gitlabPlan: ForgeReadPlan = {
			repository: { forge: "gitlab", host: "gitlab.com", projectPath: "Acme/widget" },
			artifact: { kind: "pr", number: 7 },
			include: ["files"],
			segments: [{ id: "files", command: "glab", args: ["api", "files"], shape: { kind: "ndjson" } }],
		};
		const gitlab = await executeForgeReadPlan(
			gitlabPlan,
			async () => ({
				stdout: JSON.stringify({
					new_path: "src/a.ts",
					diff: "@@ -1 +1,2 @@\n-old\n+new\n+line",
					new_file: false,
					renamed_file: false,
					deleted_file: false,
				}),
				stderr: "",
				code: 0,
				killed: false,
			}),
			"/repo",
		);
		const gitlabWindow = windowForgeRead(gitlab, { include: ["files"] });
		expect(prettyForgeReply(gitlabWindow.content, gitlabWindow.details)).toContain("src/a.ts | modified | 2 | 1");
	});

	it("fails closed on malformed maps or byte fragments and control-escapes raw display", async () => {
		const read = await executeForgeReadPlan(
			{ ...plan, include: ["artifact"], segments: [plan.segments[0]] },
			async () => ({
				stdout: JSON.stringify({ title: "Parser", body: "Body", html_url: "https://example.com" }),
				stderr: "",
				code: 0,
				killed: false,
			}),
			"/repo",
		);
		const window = windowForgeRead(read, { include: ["artifact"] });
		expect(prettyForgeReply(window.content, { ...window.details, snapshot: "bad" })).toBeNull();
		expect(
			prettyForgeReply(window.content, {
				...window.details,
				segments: [window.details.segments[0], window.details.segments[0]],
			}),
		).toBeNull();
		expect(rawForgeReply("raw\u202Espoof\u001B[31m")).toBe("raw\\u202Espoof\\u001B[31m");
	});
});
