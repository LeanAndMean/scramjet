import { createHash } from "node:crypto";
import type { ExecResult } from "@leanandmean/coding-agent";
import { describe, expect, it, vi } from "vitest";
import { deleteForgeReplyFields, filterForgeReply } from "../src/forge/filter.js";
import {
	executeForgeReadPlan,
	type ForgeWindow,
	hasCompleteSegmentCoverage,
	isForgeReadDetails,
	isForgeReadPayload,
	shellCommand,
	windowForgeRead,
} from "../src/forge/native-reply.js";
import { applyExactEdits } from "../src/forge/text.js";
import type { ForgeReadPlan, ForgeReadSegmentReceipt, ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = { forge: "github", host: "github.com", projectPath: "Acme/widget" };

function result(stdout: string, overrides: Partial<ExecResult> = {}): ExecResult {
	return { stdout, stderr: "", code: 0, killed: false, ...overrides };
}

function plan(stdoutShape: "json" | "gh-slurp" = "json"): ForgeReadPlan {
	return {
		repository,
		artifact: { kind: "issue", number: 7 },
		include: ["artifact"],
		segments: [
			{
				id: "artifact",
				command: "gh",
				args: ["api", "repos/Acme/widget/issues/7"],
				shape: { kind: stdoutShape },
				evidence: "artifact",
			},
		],
	};
}

describe("delete-only native forge filtering", () => {
	it("deletes only pinned contextual fields while preserving native names, order, nesting, and unknown fields", () => {
		const input = {
			id: 7,
			node_id: "noise",
			html_url: "https://github.com/Acme/widget/issues/7",
			future_provider_field: { keep: true },
			user: { login: "alice", avatar_url: "noise", future_user_field: 1 },
			labels: [{ name: "bug", node_id: "noise", color: "red" }],
		};
		const filtered = deleteForgeReplyFields(input, "github", "artifact");
		expect(filtered).toEqual({
			id: 7,
			html_url: "https://github.com/Acme/widget/issues/7",
			future_provider_field: { keep: true },
			user: { login: "alice", future_user_field: 1 },
			labels: [{ name: "bug", color: "red" }],
		});
		expect(deleteForgeReplyFields(filtered, "github", "artifact")).toEqual(filtered);
		expect(Object.keys(filtered as object)).toEqual(["id", "html_url", "future_provider_field", "user", "labels"]);
	});

	it("is exactly parse, delete fields, and stringify for JSON and each native NDJSON line", () => {
		const json = '{"id":7,"node_id":"x","body":"A\\nB","unknown":true}';
		expect(filterForgeReply(json, "github", "artifact")).toBe(
			JSON.stringify(deleteForgeReplyFields(JSON.parse(json), "github", "artifact")),
		);
		const note = JSON.stringify({ ...JSON.parse(json), system: false, type: null, position: null });
		const ndjson = `${note}\n${note}\n`;
		expect(filterForgeReply(ndjson, "gitlab", "comments", true)).toBe(
			ndjson
				.trim()
				.split("\n")
				.map((line) => JSON.stringify(deleteForgeReplyFields(JSON.parse(line), "gitlab", "comments")))
				.join("\n"),
		);
	});
});

describe("native forge transcript composition and windows", () => {
	it("echoes shell-quoted pinned argv and hashes the exact full transcript plus each filtered stdout", async () => {
		const raw = JSON.stringify({ number: 7, node_id: "drop", title: "It's ready", body: "A\nB" });
		const exec = vi.fn(async () => result(raw));
		const read = await executeForgeReadPlan(plan(), exec, "/repo");
		const filtered = filterForgeReply(raw, "github", "artifact");
		const transcript = `$ gh api repos/Acme/widget/issues/7\n${filtered}\n`;
		expect(read).not.toHaveProperty("transcript");
		expect(read.snapshot).toBe(createHash("sha256").update(transcript).digest("hex"));
		expect(read.segments[0].snapshot).toBe(createHash("sha256").update(filtered).digest("hex"));
		expect(exec).toHaveBeenCalledTimes(1);
		expect(shellCommand("gh", ["api", "a b", "it's"])).toBe(`gh api 'a b' 'it'"'"'s'`);
	});

	it("serves gh --slurp pages on item boundaries as valid JSON without rerunning the command", async () => {
		const pages = [
			Array.from({ length: 40 }, (_, index) => ({
				id: index + 1,
				body: `comment ${index + 1} ${"x".repeat(1800)}`,
			})),
			Array.from({ length: 10 }, (_, index) => ({ id: index + 41, body: `comment ${index + 41}` })),
		];
		const exec = vi.fn(async () => result(JSON.stringify(pages)));
		const read = await executeForgeReadPlan(plan("gh-slurp"), exec, "/repo");
		const first = windowForgeRead(read, { include: ["artifact"] });
		expect(first.truncated).toBe(true);
		expect(first.content).toContain('continue with include=["artifact"]');
		const firstOutput = Buffer.from(first.content)
			.subarray(first.details.segments[0].payload.output.start, first.details.segments[0].payload.output.end)
			.toString();
		expect(() => JSON.parse(firstOutput)).not.toThrow();
		const coverage = first.details.segments[0].coverage;
		if (coverage?.unit !== "items") throw new Error("expected item coverage");
		const second = windowForgeRead(read, { include: ["artifact"], offset: coverage.offset + coverage.count });
		expect(() =>
			JSON.parse(
				Buffer.from(second.content)
					.subarray(second.details.segments[0].payload.output.start, second.details.segments[0].payload.output.end)
					.toString(),
			),
		).not.toThrow();
		expect(exec).toHaveBeenCalledTimes(1);
	});

	it("falls back to lossless UTF-8 byte windows for one oversized item", async () => {
		const raw = JSON.stringify({ number: 7, body: "😀".repeat(40_000) });
		const read = await executeForgeReadPlan(plan(), async () => result(raw), "/repo");
		const fragments: string[] = [];
		const receipts: ForgeReadSegmentReceipt[] = [];
		let byteOffset: number | undefined;
		while (true) {
			const window = windowForgeRead(read, {
				include: ["artifact"],
				offset: 1,
				...(byteOffset === undefined ? {} : { byteOffset }),
			});
			const segment = window.details.segments[0];
			if (segment.coverage?.unit !== "bytes") throw new Error("expected byte coverage");
			fragments.push(
				Buffer.from(window.content).subarray(segment.payload.output.start, segment.payload.output.end).toString(),
			);
			receipts.push(segment);
			byteOffset = segment.coverage.offset + segment.coverage.bytes;
			if (byteOffset > segment.coverage.totalBytes) break;
		}
		expect(fragments.join("")).toBe(filterForgeReply(raw, "github", "artifact"));
		expect(hasCompleteSegmentCoverage(receipts)).toBe(true);
		expect(fragments.every((fragment) => !fragment.includes("�"))).toBe(true);
	});

	it("defers an ordinary later item to a fresh window instead of byte-fragmenting residual space", async () => {
		const twoSegments: ForgeReadPlan = {
			...plan(),
			include: ["artifact", "comments"],
			segments: [
				{ ...plan().segments[0], shape: { kind: "gh-slurp" } },
				{
					id: "comments",
					command: "gh",
					args: ["api", "comments"],
					shape: { kind: "gh-slurp" },
					evidence: "comments",
				},
			],
		};
		const outputs = [
			JSON.stringify([[{ id: 1, body: "a".repeat(42_000) }]]),
			JSON.stringify([[{ id: 2, body: "b".repeat(12_000) }]]),
		];
		let call = 0;
		const read = await executeForgeReadPlan(twoSegments, async () => result(outputs[call++]), "/repo");
		const window = windowForgeRead(read, { include: twoSegments.include });
		expect(window.details.segments).toHaveLength(1);
		expect(window.details.segments[0].coverage?.unit).toBe("items");
		expect(window.content).toContain('continue with include=["comments"] offset=1');
	});

	it("hands off from a completed oversized final item to the next segment", async () => {
		const twoSegments: ForgeReadPlan = {
			...plan(),
			include: ["artifact", "comments"],
			segments: [
				plan().segments[0],
				{
					id: "comments",
					command: "gh",
					args: ["api", "comments"],
					shape: { kind: "gh-slurp" },
					evidence: "comments",
				},
			],
		};
		let call = 0;
		const outputs = [JSON.stringify({ body: "😀".repeat(40_000) }), JSON.stringify([[{ id: 2, body: "next" }]])];
		const read = await executeForgeReadPlan(twoSegments, async () => result(outputs[call++]), "/repo");
		let byteOffset: number | undefined;
		let finalWindow: ForgeWindow;
		while (true) {
			finalWindow = windowForgeRead(read, {
				include: twoSegments.include,
				offset: 1,
				...(byteOffset === undefined ? {} : { byteOffset }),
			});
			const coverage = finalWindow.details.segments[0].coverage;
			if (coverage?.unit !== "bytes") throw new Error("expected byte coverage");
			byteOffset = coverage.offset + coverage.bytes;
			if (byteOffset > coverage.totalBytes) break;
		}
		expect(finalWindow.content).toContain('continue with include=["comments"] offset=1');
		const comments = windowForgeRead(read, { include: ["comments"] });
		expect(comments.details.segments[0].id).toBe("comments");
	});

	it("persists receipt v2, accepts zero-item comment coverage, and rejects legacy or fabricated evidence", async () => {
		const commentsPlan: ForgeReadPlan = {
			...plan(),
			include: ["comments"],
			segments: [
				{
					id: "comments",
					command: "gh",
					args: ["api", "comments"],
					shape: { kind: "gh-slurp" },
					evidence: "comments",
				},
			],
		};
		const read = await executeForgeReadPlan(commentsPlan, async () => result("[[]]"), "/repo");
		const window = windowForgeRead(read, { include: ["comments"] });
		expect(window.details).toMatchObject({ schema: "scramjet:forge-read@2", artifact: { kind: "issue", number: 7 } });
		expect(isForgeReadDetails(window.details)).toBe(true);
		expect(isForgeReadPayload(window.content, window.details)).toBe(true);
		expect(hasCompleteSegmentCoverage(window.details.segments)).toBe(true);
		expect(isForgeReadDetails({ ...window.details, schema: "scramjet:forge-read@1" })).toBe(false);
		expect(isForgeReadDetails({ ...window.details, snapshot: "bad" })).toBe(false);
		const artifactRead = await executeForgeReadPlan(plan(), async () => result('{"number":7}'), "/repo");
		const artifactWindow = windowForgeRead(artifactRead, { include: ["artifact"] });
		expect(
			isForgeReadDetails({
				...artifactWindow.details,
				segments: [{ ...artifactWindow.details.segments[0], id: "files", evidence: "artifact" }],
			}),
		).toBe(false);
		expect(
			isForgeReadDetails({
				...artifactWindow.details,
				segments: [artifactWindow.details.segments[0], artifactWindow.details.segments[0]],
			}),
		).toBe(false);
		expect(isForgeReadDetails({ ...artifactWindow.details, segments: [] })).toBe(false);
		expect(
			isForgeReadDetails({
				...artifactWindow.details,
				segments: [
					{
						...artifactWindow.details.segments[0],
						payload: {
							...artifactWindow.details.segments[0].payload,
							segment: { ...artifactWindow.details.segments[0].payload.segment, start: 1 },
						},
					},
				],
			}),
		).toBe(false);
		expect(isForgeReadPayload("", artifactWindow.details)).toBe(false);
	});
});

describe("applyExactEdits", () => {
	it("matches decoded values exactly and computes all replacements against one original", () => {
		expect(
			applyExactEdits(
				"alpha\tbeta gamma",
				[
					{ oldText: "\t", newText: " " },
					{ oldText: "gamma", newText: "G" },
				],
				"body",
			),
		).toBe("alpha beta G");
		expect(() => applyExactEdits("café\r\n—", [{ oldText: "cafe\n-", newText: "x" }], "body")).toThrow(
			/not found exactly/,
		);
	});

	it.each([
		["abc", [], /at least one edit/],
		["abc", [{ oldText: "", newText: "x" }], /oldText must not be empty/],
		["abc abc", [{ oldText: "abc", newText: "x" }], /not unique/],
		["abc", [{ oldText: "abc", newText: "abc" }], /no-op/],
		[
			"abcdef",
			[
				{ oldText: "abc", newText: "x" },
				{ oldText: "bcd", newText: "y" },
			],
			/overlap/,
		],
	] as const)("rejects invalid edit sets", (original, edits, message) => {
		expect(() => applyExactEdits(original, edits, "field")).toThrow(message);
	});
});
