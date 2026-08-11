import { initTheme } from "@leanandmean/coding-agent";
import { visibleWidth } from "@leanandmean/tui";
import { describe, expect, it } from "vitest";
import { executeForgeReadPlan, windowForgeRead } from "../src/forge/native-reply.js";
import { ForgeReplyComponent, rawForgeReply } from "../src/forge/renderer.js";
import { controlSafeText, losslessControlSafeText } from "../src/forge/text.js";
import type { ForgeReadPlan } from "../src/forge/types.js";

initTheme(undefined, false);

const theme = {
	fg: (_color: string, value: string) => value,
	bold: (value: string) => value,
} as never;

const githubPlan: ForgeReadPlan = {
	repository: { forge: "github", host: "github.com", projectPath: "Acme/widget" },
	artifact: { kind: "issue", number: 7 },
	include: ["artifact", "comments"],
	segments: [
		{ id: "artifact", command: "gh", args: ["api", "issue"], shape: { kind: "json" }, evidence: "artifact" },
		{ id: "comments", command: "gh", args: ["api", "comments"], shape: { kind: "gh-slurp" }, evidence: "comments" },
	],
};

async function readWindow(plan: ForgeReadPlan, outputs: string[], limit?: number) {
	let index = 0;
	const read = await executeForgeReadPlan(
		plan,
		async () => ({ stdout: outputs[index++], stderr: "", code: 0, killed: false }),
		"/repo",
	);
	return windowForgeRead(read, { include: plan.include, ...(limit === undefined ? {} : { limit }) });
}

function render(
	content: string,
	details: unknown,
	width = 100,
): { component: ForgeReplyComponent; text: string; lines: string[] } {
	const component = new ForgeReplyComponent();
	component.update(content, details, { expanded: true, isPartial: false }, theme);
	const lines = component.render(width);
	return { component, text: lines.join("\n"), lines };
}

function readable(text: string): string {
	const start = text.indexOf("Readable view");
	return start === -1 ? "" : text.slice(start);
}

function rawSection(text: string): string {
	const start = text.indexOf("Raw transcript");
	const end = text.indexOf("Readable view");
	return start === -1 ? "" : text.slice(start, end === -1 ? undefined : end);
}

function replaceFirstOutput(
	window: Awaited<ReturnType<typeof readWindow>>,
	output: string,
): { content: string; details: typeof window.details } {
	const details = structuredClone(window.details);
	const segment = details.segments[0];
	const source = Buffer.from(window.content, "utf8");
	const prefix = source.subarray(0, segment.payload.output.start).toString("utf8");
	const suffix = source.subarray(segment.payload.output.end).toString("utf8");
	const delta = Buffer.byteLength(output, "utf8") - (segment.payload.output.end - segment.payload.output.start);
	segment.payload.output.end += delta;
	segment.payload.segment.end += delta;
	return { content: `${prefix}${output}${suffix}`, details };
}

describe("forge persisted-reply renderer", () => {
	it("renders GitHub Markdown bodies and every retained native metadata value", async () => {
		const body = "# Body heading\n\nA **bold** paragraph.\n\n- first item\n\n`code`\n\n<!-- body marker -->";
		const commentBody = "## Comment heading\n\n[link](https://example.com)\u202Espoof";
		const window = await readWindow(
			githubPlan,
			[
				JSON.stringify({
					title: "Parser",
					body,
					state: "open",
					html_url: "https://github.com/Acme/widget/issues/7",
					future_provider_field: { nested: [true, 17, null, "artifact sentinel"] },
				}),
				JSON.stringify([
					[
						{
							id: 1,
							body: commentBody,
							user: { login: "alice", future_actor_field: "actor sentinel" },
							future_comment_field: { deep: "comment sentinel" },
						},
						{ id: 2, body: "", user: { login: "bob" }, empty_sentinel: false },
						{ id: 3, body: "third", user: { login: "carol" } },
					],
				]),
			],
			2,
		);
		const rendered = render(window.content, window.details);
		const human = readable(rendered.text);
		expect(human).toContain("Body heading");
		expect(human).toContain("bold");
		expect(human).not.toContain("**bold**");
		expect(human).toContain("first item");
		expect(human).not.toContain("\n- first item");
		expect(human).toContain("body marker");
		expect(human).toContain("Comment heading");
		expect(human).toContain('"state": "open"');
		expect(human).toContain('"html_url": "https://github.com/Acme/widget/issues/7"');
		expect(human).toContain('"id": 1');
		expect(human).toContain('"login": "alice"');
		expect(human).toContain("artifact sentinel");
		expect(human).toContain("actor sentinel");
		expect(human).toContain("comment sentinel");
		expect(human).toContain('"empty_sentinel": false');
		expect(human).toContain("(empty body)");
		expect(human).toContain("Continuation");
		expect(human).toContain("continue with include=");
		expect(rendered.text).toContain("Raw transcript");
		const raw = rawSection(rendered.text);
		expect(raw).toContain("$ gh api issue");
		expect(raw).toContain('"future_provider_field"');
		expect(raw).toContain("artifact sentinel");
		expect(raw).toContain("continue with include=");
		expect(rendered.text).not.toContain("\u202E");
		expect(rendered.text).toContain("\\u202E");
	});

	it("renders GitLab descriptions, NDJSON comments, and complete native metadata", async () => {
		const plan: ForgeReadPlan = {
			repository: { forge: "gitlab", host: "gitlab.com", projectPath: "Acme/widget" },
			artifact: { kind: "pr", number: 12 },
			include: ["artifact", "comments"],
			segments: [
				{ id: "artifact", command: "glab", args: ["api", "mr"], shape: { kind: "json" }, evidence: "artifact" },
				{
					id: "comments",
					command: "glab",
					args: ["api", "notes"],
					shape: { kind: "ndjson" },
					evidence: "comments",
				},
			],
		};
		const window = await readWindow(plan, [
			JSON.stringify({
				title: "Ship",
				description: "# Description\n\n**ready**",
				web_url: "https://gitlab.com/Acme/widget/-/merge_requests/12",
				future_mr: { count: 3 },
			}),
			[
				JSON.stringify({
					id: 1,
					body: "- note item",
					author: { username: "alice" },
					future_note: [1, null],
					system: false,
					type: null,
					position: null,
				}),
				JSON.stringify({
					id: 2,
					body: null,
					author: null,
					nullable_sentinel: true,
					system: false,
					type: null,
					position: null,
				}),
			].join("\n"),
		]);
		const human = readable(render(window.content, window.details).text);
		expect(human).toContain("Description");
		expect(human).toContain("ready");
		expect(human).not.toContain("**ready**");
		expect(human).toContain("note item");
		expect(human).not.toContain("\n- note item");
		expect(human).toContain('"future_mr"');
		expect(human).toContain('"future_note"');
		expect(human).toContain('"body": null');
		expect(human).toContain('"nullable_sentinel": true');
	});

	it("keeps complete non-body facet values instead of lossy tables", async () => {
		const plan: ForgeReadPlan = {
			...githubPlan,
			include: ["commits"],
			segments: [{ id: "commits", command: "gh", args: ["api", "commits"], shape: { kind: "gh-slurp" } }],
		};
		const window = await readWindow(plan, [
			JSON.stringify([
				[
					{
						sha: "abcdef1234567890",
						commit: {
							message: "First line\ncomplete body sentinel",
							author: { name: "Alice", date: "2026-08-11", future: { nested: 9 } },
						},
						future_commit_field: "retained",
					},
				],
			]),
		]);
		const human = readable(render(window.content, window.details).text);
		expect(human).toContain("abcdef1234567890");
		expect(human).toContain("complete body sentinel");
		expect(human).toContain("future_commit_field");
		expect(human).toContain('"nested": 9');
	});

	it("renders valid optional errors beside readable successful segments", async () => {
		const plan: ForgeReadPlan = {
			...githubPlan,
			include: ["artifact", "parent"],
			segments: [
				githubPlan.segments[0],
				{ id: "parent", command: "gh", args: ["api", "parent"], shape: { kind: "json" }, optional: true },
			],
		};
		let call = 0;
		const read = await executeForgeReadPlan(
			plan,
			async () =>
				call++ === 0
					? { stdout: JSON.stringify({ title: "Parser", body: "**body**" }), stderr: "", code: 0, killed: false }
					: { stdout: "", stderr: "gh: No parent issue found (HTTP 404)", code: 1, killed: false },
			"/repo",
		);
		const window = windowForgeRead(read, { include: plan.include });
		const human = readable(render(window.content, window.details).text);
		expect(human).toContain("body");
		expect(human).toContain("Provider error");
		expect(human).toContain("No parent issue found");
	});

	it("fails closed on malformed payloads, outer shapes, body fields, and receipts", async () => {
		const oneSegment: ForgeReadPlan = {
			...githubPlan,
			include: ["artifact"],
			segments: [githubPlan.segments[0]],
		};
		const valid = await readWindow(oneSegment, [JSON.stringify({ title: "Parser", body: "Body" })]);
		const malformedJson = replaceFirstOutput(valid, "{not-json");
		expect(render(malformedJson.content, malformedJson.details).text).not.toContain("Readable view");

		const invalidBody = await readWindow(oneSegment, [JSON.stringify({ title: "Parser", body: 7 })]);
		expect(render(invalidBody.content, invalidBody.details).text).not.toContain("Readable view");

		const githubComments: ForgeReadPlan = {
			...githubPlan,
			include: ["comments"],
			segments: [{ ...githubPlan.segments[1], shape: { kind: "json" } }],
		};
		const malformedPages = await readWindow(githubComments, [JSON.stringify({ id: 1, body: "not a page" })]);
		expect(render(malformedPages.content, malformedPages.details).text).not.toContain("Readable view");

		const gitlabComments: ForgeReadPlan = {
			repository: { forge: "gitlab", host: "gitlab.com", projectPath: "Acme/widget" },
			artifact: { kind: "issue", number: 7 },
			include: ["comments"],
			segments: [
				{ id: "comments", command: "glab", args: ["api", "notes"], shape: { kind: "json" }, evidence: "comments" },
			],
		};
		const malformedNdjson = await readWindow(gitlabComments, [JSON.stringify("not a record")]);
		expect(render(malformedNdjson.content, malformedNdjson.details).text).not.toContain("Readable view");

		const invalidDetails = { ...valid.details, snapshot: "bad" };
		const invalidReceipt = render(valid.content, invalidDetails).text;
		expect(invalidReceipt).not.toContain("Readable view");
		expect(invalidReceipt).toContain("$ gh api issue");
		const collapsed = new ForgeReplyComponent();
		collapsed.update(valid.content, invalidDetails, { expanded: false, isPartial: false }, theme);
		const collapsedText = collapsed.render(80).join("\n");
		expect(collapsedText).toContain("forge read result unavailable");
		expect(collapsedText).not.toContain("$ gh api issue");
	});

	it("fails closed to reversible control-safe raw output", async () => {
		const raw = JSON.stringify({ title: "Parser", body: "😀".repeat(40_000) });
		const byteWindow = windowForgeRead(
			await executeForgeReadPlan(
				{ ...githubPlan, include: ["artifact"], segments: [githubPlan.segments[0]] },
				async () => ({ stdout: raw, stderr: "", code: 0, killed: false }),
				"/repo",
			),
			{ include: ["artifact"] },
		);
		const byteRendered = render(byteWindow.content, byteWindow.details).text;
		expect(byteRendered).toContain("Raw transcript");
		expect(byteRendered).not.toContain("Readable view");
		expect(rawSection(byteRendered)).toContain("$ gh api issue");
		expect(rawForgeReply("raw\u202Espoof\u001B[31m")).toBe("raw\\u202Espoof\\u001B[31m");
		expect(losslessControlSafeText("literal\\u001B actual\u001B\t\r\u206A\uFFF9")).toBe(
			"literal\\\\u001B actual\\u001B\\t\\r\\u206A\\uFFF9",
		);
		expect(controlSafeText("readable\u009B\u202E\u206A\uFFF9\uFDD0\u{1FFFE}")).toBe(
			"readable\\u009B\\u202E\\u206A\\uFFF9\\uFDD0\\uD83F\\uDFFE",
		);
	});

	it("reflows safely at narrow widths and rebuilds after invalidation", async () => {
		const window = await readWindow(githubPlan, [
			JSON.stringify({ title: "Parser", body: "# Heading\n\nA long paragraph with a_unique_body_sentinel" }),
			JSON.stringify([[{ id: 1, body: "comment sentinel", future: "metadata sentinel" }]]),
		]);
		const rendered = render(window.content, window.details, 24);
		expect(rendered.lines.every((line) => visibleWidth(line) <= 24)).toBe(true);
		expect(rendered.text).toContain("a_unique_body_sentinel");
		const wideBeforeThemeChange = rendered.component.render(100).join("\n");
		expect(rendered.lines.length).toBeGreaterThan(wideBeforeThemeChange.split("\n").length);
		try {
			initTheme("pi-light", false);
			rendered.component.invalidate();
			const wideAfterThemeChange = rendered.component.render(100).join("\n");
			expect(wideAfterThemeChange).toContain("metadata sentinel");
			expect(wideAfterThemeChange).not.toBe(wideBeforeThemeChange);
		} finally {
			initTheme(undefined, false);
		}
	});
});
