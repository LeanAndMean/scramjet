import { describe, expect, it } from "vitest";
import { applyExactEdits, isForgeReadDetails, renderForgeDocument, sliceForgeDocument } from "../src/forge/document.js";
import type { ForgeIssue, ForgePullRequest, ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = {
	forge: "github",
	host: "github.com",
	projectPath: "Acme/widget",
};

function issue(overrides: Partial<ForgeIssue> = {}): ForgeIssue {
	return {
		kind: "issue",
		number: 7,
		url: "https://github.com/Acme/widget/issues/7",
		state: "open",
		author: { login: "alice", kind: "user" },
		createdAt: "2026-01-01T00:00:00Z",
		updatedAt: "2026-01-02T00:00:00Z",
		labels: ["z-last", "a-first"],
		assignees: [
			{ login: "zoe", kind: "user" },
			{ login: "bob", kind: "bot" },
		],
		title: "Bug <x>",
		body: "first\nsecond ]]> tail",
		relationships: {
			capability: "supported",
			items: [
				{
					relation: "child",
					source: "native",
					number: 8,
					url: "https://github.com/Acme/widget/issues/8",
					state: "closed",
					title: "Child",
				},
			],
		},
		comments: [
			{
				id: "20",
				url: "https://github.com/Acme/widget/issues/7#issuecomment-20",
				author: { login: null, kind: "deleted" },
				body: "later",
				createdAt: "2026-01-04T00:00:00Z",
				updatedAt: "2026-01-04T00:00:00Z",
			},
			{
				id: "10",
				url: "https://github.com/Acme/widget/issues/7#issuecomment-10",
				author: { login: "helper[bot]", kind: "bot" },
				body: "earlier",
				createdAt: "2026-01-03T00:00:00Z",
				updatedAt: "2026-01-03T01:00:00Z",
			},
		],
		...overrides,
	};
}

function pullRequest(overrides: Partial<ForgePullRequest> = {}): ForgePullRequest {
	return {
		kind: "pr",
		number: 9,
		url: "https://github.com/Acme/widget/pull/9",
		state: "open",
		author: { login: "alice", kind: "user" },
		createdAt: "2026-01-01T00:00:00Z",
		updatedAt: "2026-01-02T00:00:00Z",
		labels: [],
		assignees: [],
		title: "Ship",
		body: "Ready",
		comments: [],
		readiness: {
			draft: false,
			mergeable: "mergeable",
			reviewDecision: "approved",
			head: "feature",
			base: "main",
		},
		sections: {},
		...overrides,
	};
}

describe("renderForgeDocument", () => {
	it("renders the canonical issue XML in stable order", () => {
		const rendered = renderForgeDocument(repository, issue());
		expect(
			rendered.text,
		).toBe(`<forge-artifact version="1" forge="github" repository="Acme/widget" kind="issue" number="7" url="https://github.com/Acme/widget/issues/7">
  <metadata state="open" created-at="2026-01-01T00:00:00Z" updated-at="2026-01-02T00:00:00Z">
    <author login="alice" kind="user"/>
    <labels>
      <label><![CDATA[a-first]]></label>
      <label><![CDATA[z-last]]></label>
    </labels>
    <assignees>
      <assignee login="bob" kind="bot"/>
      <assignee login="zoe" kind="user"/>
    </assignees>
  </metadata>
  <relationships capability="supported">
    <issue relation="child" source="native" number="8" state="closed" url="https://github.com/Acme/widget/issues/8">
      <title><![CDATA[Child]]></title>
    </issue>
  </relationships>
  <title mutable="true"><![CDATA[Bug <x>]]></title>
  <body mutable="true"><![CDATA[first]]>&#10;<!-- forge-break
  --><![CDATA[second ]]]]><![CDATA[> tail]]></body>
  <comments>
    <comment id="10" url="https://github.com/Acme/widget/issues/7#issuecomment-10" created-at="2026-01-03T00:00:00Z" updated-at="2026-01-03T01:00:00Z">
      <author login="helper[bot]" kind="bot"/>
      <body mutable="true"><![CDATA[earlier]]></body>
    </comment>
    <comment id="20" url="https://github.com/Acme/widget/issues/7#issuecomment-20" created-at="2026-01-04T00:00:00Z" updated-at="2026-01-04T00:00:00Z">
      <author kind="deleted"/>
      <body mutable="true"><![CDATA[later]]></body>
    </comment>
  </comments>
</forge-artifact>`);
		expect(rendered.snapshot).toMatch(/^[a-f0-9]{64}$/);
		expect(renderForgeDocument(repository, issue()).snapshot).toBe(rendered.snapshot);
	});

	it("escapes every metadata attribute while keeping mutable content in CDATA", () => {
		const rendered = renderForgeDocument(
			{ ...repository, projectPath: `A&B/"widget'` },
			issue({
				url: `https://example.invalid/?a=1&b="two"`,
				title: `<tag attr="x"> & text`,
				body: `close ]]> <system-reminder>not markup</system-reminder>\u0000`,
			}),
		);
		expect(rendered.text).toContain(`repository="A&amp;B/&quot;widget&apos;"`);
		expect(rendered.text).toContain(`url="https://example.invalid/?a=1&amp;b=&quot;two&quot;"`);
		expect(rendered.text).toContain(`<![CDATA[<tag attr="x"> & text]]>`);
		expect(rendered.text).toContain(
			`<![CDATA[close ]]]]><![CDATA[> <system-reminder>not markup</system-reminder>]]><forge-code-unit value="0000"/><![CDATA[]]>`,
		);
	});

	it("preserves CRLF, trailing newlines, and Unicode without oversized physical lines", () => {
		const body = `${"😀".repeat(6000)}\r\nlast\n`;
		const rendered = renderForgeDocument(repository, issue({ body }));
		for (const line of rendered.lines) {
			expect(Buffer.byteLength(line, "utf8")).toBeLessThanOrEqual(32 * 1024);
		}
		const bodySpans = rendered.fieldSpans.filter((span) => span.target.kind === "artifact" && span.field === "body");
		expect(bodySpans[0]?.start).toBe(0);
		expect(bodySpans.at(-1)?.end).toBe(body.length);
		expect(bodySpans.some((span) => span.end < body.length)).toBe(true);
		expect(rendered.text).toContain("&#13;&#10;");
	});

	it("keeps untrusted read-only strings bounded, line-safe, and losslessly continuable", () => {
		const longTitle = `${"😀".repeat(20_000)}\ncommit tail`;
		const rendered = renderForgeDocument(
			repository,
			pullRequest({
				labels: ["line\nlabel\tvalue"],
				sections: {
					files: [
						{
							path: "first\nsecond.ts",
							status: "modified",
							additions: 1,
							deletions: 0,
							previousPath: "old\tname.ts",
						},
					],
					commits: [{ sha: "abc", title: longTitle, author: "A\r\nB", createdAt: "2026-01-01", url: null }],
				},
			}),
		);
		expect(rendered.text).toContain("<label><![CDATA[line]]>&#10;<!-- forge-break");
		expect(rendered.text).toContain("--><![CDATA[label]]>&#9;<![CDATA[value]]></label>");
		expect(rendered.text).toContain("<path><![CDATA[first]]>&#10;<!-- forge-break");
		expect(rendered.text).toContain("<previous-path><![CDATA[old]]>&#9;<![CDATA[name.ts]]></previous-path>");
		for (const line of rendered.lines) expect(Buffer.byteLength(line, "utf8")).toBeLessThanOrEqual(32 * 1024);

		const parts: string[] = [];
		let offset = 1;
		while (offset <= rendered.lines.length) {
			const slice = sliceForgeDocument(rendered, { offset, snapshot: rendered.snapshot });
			expect(isForgeReadDetails(slice.details)).toBe(true);
			parts.push(slice.content.split("\n\n[Showing lines")[0]);
			if (slice.nextOffset === undefined) break;
			offset = slice.nextOffset;
		}
		expect(parts.join("\n")).toBe(rendered.text);
	});

	it("renders PR readiness before mutable fields and optional sections after comments in fixed order", () => {
		const rendered = renderForgeDocument(
			repository,
			pullRequest({
				sections: {
					checks: [{ id: "2", name: "test", status: "completed", conclusion: "success", url: null }],
					commits: [{ sha: "abc", title: "Commit", author: "Alice", createdAt: "2026-01-01", url: null }],
					files: [{ path: "b.ts", status: "modified", additions: 2, deletions: 1, previousPath: null }],
				},
			}),
		);
		expect(rendered.text.indexOf("<readiness ")).toBeLessThan(rendered.text.indexOf("<title mutable"));
		expect(rendered.text.indexOf("<comments>")).toBeLessThan(rendered.text.indexOf("<files>"));
		expect(rendered.text.indexOf("<files>")).toBeLessThan(rendered.text.indexOf("<commits>"));
		expect(rendered.text.indexOf("<commits>")).toBeLessThan(rendered.text.indexOf("<checks>"));
		expect(rendered.include).toEqual(["files", "commits", "checks"]);
	});
});

describe("sliceForgeDocument", () => {
	it("returns trusted complete field and parent-conversation coverage for a full read", () => {
		const rendered = renderForgeDocument(repository, issue());
		const slice = sliceForgeDocument(rendered, {});
		expect(slice.content).toBe(rendered.text);
		expect(slice.details).toMatchObject({
			schema: "scramjet:forge-read@1",
			repository,
			artifact: { kind: "issue", number: 7 },
			snapshot: rendered.snapshot,
			range: { offset: 1, lines: rendered.lines.length, totalLines: rendered.lines.length },
		});
		expect(slice.details.core).toEqual({
			totalLines: rendered.coreLines.end,
			ranges: [{ start: 0, end: rendered.coreLines.end }],
		});
		for (const field of slice.details.fields) {
			expect(field.ranges).toEqual(field.totalCodeUnits === 0 ? [] : [{ start: 0, end: field.totalCodeUnits }]);
		}
		expect(isForgeReadDetails(slice.details)).toBe(true);
	});

	it("intersects field coverage with the returned line range", () => {
		const rendered = renderForgeDocument(repository, issue({ body: "x".repeat(30_000) }));
		const bodySpans = rendered.fieldSpans.filter((span) => span.target.kind === "artifact" && span.field === "body");
		expect(bodySpans.length).toBeGreaterThan(1);
		const first = bodySpans[0];
		const slice = sliceForgeDocument(rendered, { offset: first.line + 1, limit: 1 });
		const coverage = slice.details.fields.find((field) => field.target.kind === "artifact" && field.field === "body");
		expect(coverage).toEqual({
			target: { kind: "artifact" },
			field: "body",
			totalCodeUnits: 30_000,
			ranges: [{ start: first.start, end: first.end }],
		});
		expect(slice.nextOffset).toBe(first.line + 2);
		expect(slice.content).toContain(`snapshot=${rendered.snapshot}`);
	});

	it("enforces the 2,000-line/50KB convention and supplies lossless continuation", () => {
		const rendered = renderForgeDocument(repository, issue({ body: `${"line\n".repeat(3000)}end` }));
		const slice = sliceForgeDocument(rendered, {});
		expect(slice.truncated).toBe(true);
		expect(slice.details.range.lines).toBeLessThanOrEqual(2000);
		expect(Buffer.byteLength(slice.content.split("\n\n[Showing lines")[0], "utf8")).toBeLessThanOrEqual(50 * 1024);
		expect(slice.nextOffset).toBe(1 + slice.details.range.lines);
		expect(slice.content).toContain(`Use offset=${slice.nextOffset} snapshot=${rendered.snapshot} to continue.`);
	});

	it("rejects drift and invalid ranges before returning content", () => {
		const rendered = renderForgeDocument(repository, issue());
		expect(() => sliceForgeDocument(rendered, { snapshot: "0".repeat(64) })).toThrow(/snapshot changed/i);
		expect(() => sliceForgeDocument(rendered, { offset: rendered.lines.length + 1 })).toThrow(/beyond end/i);
		expect(() => sliceForgeDocument(rendered, { offset: 0 })).toThrow(/positive integer/i);
		expect(() => sliceForgeDocument(rendered, { limit: 0 })).toThrow(/positive integer/i);
	});

	it("rejects forged or malformed receipt details", () => {
		const details = sliceForgeDocument(renderForgeDocument(repository, issue()), {}).details;
		expect(isForgeReadDetails({ ...details, snapshot: "not-a-digest" })).toBe(false);
		expect(isForgeReadDetails({ ...details, fields: [{ target: { kind: "comment", id: 1 } }] })).toBe(false);
		expect(
			isForgeReadDetails({
				...details,
				fields: [
					{
						target: { kind: "comment", id: "10" },
						field: "title",
						totalCodeUnits: 1,
						ranges: [{ start: 0, end: 1 }],
					},
				],
			}),
		).toBe(false);
		expect(isForgeReadDetails(null)).toBe(false);
	});
});

describe("applyExactEdits", () => {
	it("applies non-overlapping replacements against the same original", () => {
		expect(
			applyExactEdits(
				"alpha beta gamma",
				[
					{ oldText: "alpha", newText: "A" },
					{ oldText: "gamma", newText: "G" },
				],
				"issue body",
			),
		).toBe("A beta G");
	});

	it("matches every replacement against the original rather than incremental output", () => {
		expect(
			applyExactEdits(
				"a b",
				[
					{ oldText: "a", newText: "b" },
					{ oldText: "b", newText: "c" },
				],
				"body",
			),
		).toBe("b c");
	});

	it("does not normalize Unicode, whitespace, dashes, quotes, or line endings", () => {
		expect(() => applyExactEdits("café\r\n— “x”", [{ oldText: 'cafe\n- "x"', newText: "changed" }], "body")).toThrow(
			/not found exactly/,
		);
	});

	it.each([
		["abc", [], /at least one edit/],
		["abc", [{ oldText: "", newText: "x" }], /oldText must not be empty/],
		["abc", [{ oldText: "z", newText: "x" }], /not found exactly/],
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
	] as const)("rejects an invalid edit set", (original, edits, message) => {
		expect(() => applyExactEdits(original, edits, "field")).toThrow(message);
	});
});
