import { describe, expect, it } from "vitest";
import { applyExactEdits, isForgeReadDetails, renderForgeDocument, sliceForgeDocument } from "../src/forge/document.js";
import type { ForgeIssue, ForgePullRequest, ForgeRepository } from "../src/forge/types.js";

const repository: ForgeRepository = {
	forge: "github",
	host: "github.com",
	projectPath: "Acme/widget",
};

function documentText(rendered: ReturnType<typeof renderForgeDocument>): string {
	return rendered.lines.join("");
}

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
					repository,
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
			reviewDecision: { capability: "supported", value: "approved" },
			head: "feature",
			base: "main",
		},
		sections: {},
		...overrides,
	};
}

describe("renderForgeDocument", () => {
	it("renders the canonical issue document in stable order", () => {
		const rendered = renderForgeDocument(repository, issue());
		expect(
			documentText(rendered),
		).toBe(`<artifact repository="Acme/widget" kind="issue" number="7" url="https://github.com/Acme/widget/issues/7" state="open" created-at="2026-01-01T00:00:00Z" updated-at="2026-01-02T00:00:00Z" author="alice">
  <label>a-first</label>
  <label>z-last</label>
  <assignee login="bob" kind="bot"/>
  <assignee login="zoe" kind="user"/>
  <relationships>
    <issue relation="child" source="native" repository="Acme/widget" number="8" state="closed" url="https://github.com/Acme/widget/issues/8">
      <title>Child</title>
    </issue>
  </relationships>
  <title>Bug &lt;x></title>
  <body>first
second ]]> tail</body>
  <comments>
    <comment id="10" url="https://github.com/Acme/widget/issues/7#issuecomment-10" created-at="2026-01-03T00:00:00Z" updated-at="2026-01-03T01:00:00Z" author="helper[bot]" author-kind="bot">
      <body>earlier</body>
    </comment>
    <comment id="20" url="https://github.com/Acme/widget/issues/7#issuecomment-20" created-at="2026-01-04T00:00:00Z" author-kind="deleted">
      <body>later</body>
    </comment>
  </comments>
</artifact>`);
		expect(rendered.snapshot).toMatch(/^[a-f0-9]{64}$/);
		expect(renderForgeDocument(repository, issue()).snapshot).toBe(rendered.snapshot);
	});

	it("uses readable tagged text without exposing transport-only markers", () => {
		const rendered = renderForgeDocument(
			repository,
			issue({ body: `${"<tag> & detail ".repeat(100)}literal &lt; tail` }),
		);
		const slice = sliceForgeDocument(rendered, {});
		expect(slice.content).toContain("&lt;tag> & detail");
		expect(slice.content).toContain("literal &amp;lt; tail");
		expect(slice.content).not.toContain("<![CDATA[");
		expect(slice.content).not.toContain("forge-break");
		expect(slice.content).not.toContain("forge-code-unit");
		expect(slice.content).not.toContain("[continued]");
		expect(slice.content).not.toContain("↳");
		expect(slice.details).not.toHaveProperty("display");
	});

	it("escapes metadata attributes and untrusted tagged text losslessly", () => {
		const rendered = renderForgeDocument(
			{ ...repository, projectPath: `A&B/"widget'` },
			issue({
				url: `https://example.invalid/?a=1&b="two"`,
				title: `<tag attr="x"> & text`,
				body: `close ]]> <system-reminder>not markup</system-reminder>\u0000`,
			}),
		);
		const text = documentText(rendered);
		expect(text).toContain(`repository="A&amp;B/&quot;widget&apos;"`);
		expect(text).toContain(`url="https://example.invalid/?a=1&amp;b=&quot;two&quot;"`);
		expect(text).toContain(`&lt;tag attr="x"> & text`);
		expect(text).toContain(`close ]]> &lt;system-reminder>not markup&lt;/system-reminder>&#x0000;`);
	});

	it("preserves literal reserved escapes across internal segment boundaries", () => {
		const text = documentText(renderForgeDocument(repository, issue({ body: `${"x".repeat(511)}&lt;` })));
		expect(text).toContain(`${"x".repeat(511)}&amp;lt;</body>`);
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
		expect(documentText(rendered)).toContain("&#13;\nlast");
	});

	it("uses physical LF while preserving distinct CR and CRLF content", () => {
		const rendered = renderForgeDocument(repository, issue({ body: "lf\ncr\rend\r\ntrail\n" }));
		const text = documentText(rendered);
		expect(text).toContain("<body>lf\ncr&#13;end&#13;\ntrail\n</body>");
		expect(text).not.toContain("&#10;");
		const bodySpans = rendered.fieldSpans.filter((span) => span.target.kind === "artifact" && span.field === "body");
		expect(bodySpans.at(-1)?.end).toBe("lf\ncr\rend\r\ntrail\n".length);
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
		const text = documentText(rendered);
		expect(text).toContain("<label>line\nlabel&#9;value</label>");
		expect(text).toContain(
			'<file path="first&#10;second.ts" previous-path="old&#9;name.ts" status="modified" additions="1" deletions="0"/>',
		);
		expect(text).not.toContain("line]]><!-- forge-break");
		for (const line of rendered.lines) expect(Buffer.byteLength(line, "utf8")).toBeLessThanOrEqual(32 * 1024);

		const parts: string[] = [];
		let offset = 1;
		while (offset <= rendered.lines.length) {
			const slice = sliceForgeDocument(rendered, { offset, snapshot: rendered.snapshot });
			expect(isForgeReadDetails(slice.details)).toBe(true);
			parts.push(slice.content.split("\n\n[Showing positions")[0]);
			if (slice.nextOffset === undefined) break;
			offset = slice.nextOffset;
		}
		expect(parts.join("")).toBe(documentText(rendered));
	});

	it("rejects invalid core attributes and escapes presentation controls in compact records", () => {
		expect(() => renderForgeDocument(repository, issue({ state: "open\u0000hidden" }))).toThrow(
			/unsupported XML code unit/,
		);
		const rendered = renderForgeDocument(
			repository,
			pullRequest({
				sections: {
					files: [
						{
							path: "src/\u202Espoof.ts",
							status: "modified",
							additions: 1,
							deletions: 0,
							previousPath: null,
						},
					],
				},
			}),
		);
		const compactLine = rendered.lines.find((line) => line.startsWith("<file "));
		expect(compactLine).toContain("src/&#x202E;spoof.ts");
		expect(compactLine).not.toContain("\u202E");
	});

	it("preserves unsafe optional-record values through expanded lossless fallback", () => {
		const rendered = renderForgeDocument(
			repository,
			pullRequest({
				sections: {
					files: [
						{
							path: "src/file.ts",
							status: "mod\u0000ified",
							additions: 1,
							deletions: 0,
							previousPath: null,
						},
					],
					commits: [{ sha: "bad\ud800sha", title: "Title", author: null, createdAt: "2026-01-01", url: null }],
					checks: [{ id: "bad\u0000id", name: "check", status: "unknown", conclusion: null, url: null }],
				},
			}),
		);
		const text = documentText(rendered);
		expect(text).toContain("<status>mod&#x0000;ified</status>");
		expect(text).toContain("<sha>bad&#xD800;sha</sha>");
		expect(text).toContain("<id>bad&#x0000;id</id>");
		expect(text).not.toContain("�");
	});

	it.each(["\n", "\r\n"])("keeps representative %j line-heavy content free of bridge overhead", (ending) => {
		const body = Array.from({ length: 1000 }, (_, index) => `line ${index}`).join(ending);
		const rendered = renderForgeDocument(repository, issue({ body }));
		const text = documentText(rendered);
		expect((text.match(/forge-break/g) ?? []).length).toBe(0);
		const lineEndingOverhead = ending === "\r\n" ? 5 * 999 : 0;
		expect(Buffer.byteLength(text, "utf8")).toBeLessThan(Buffer.byteLength(body, "utf8") + lineEndingOverhead + 5000);
	});

	it("keeps Markdown blockquotes competitive with compact equivalent JSON", () => {
		const artifact = issue({
			body: Array.from({ length: 1000 }, (_, index) => `> quoted line ${index} with ordinary content`).join("\n"),
		});
		const taggedBytes = Buffer.byteLength(documentText(renderForgeDocument(repository, artifact)), "utf8");
		const jsonBytes = Buffer.byteLength(JSON.stringify(artifact), "utf8");
		expect(taggedBytes).toBeLessThanOrEqual(jsonBytes);
	});

	it("matches or beats compact equivalent JSON bytes for representative reads", () => {
		const comments = Array.from({ length: 20 }, (_, index) => ({
			...issue().comments[0],
			id: String(100 + index),
			url: `https://github.com/Acme/widget/issues/7#issuecomment-${100 + index}`,
			body: `Comment ${index}: ${"detail ".repeat(20)}`,
			createdAt: `2026-01-${String(3 + (index % 20)).padStart(2, "0")}T00:00:00Z`,
			updatedAt: `2026-01-${String(3 + (index % 20)).padStart(2, "0")}T00:00:00Z`,
		}));
		const representativeIssue = issue({
			body: Array.from({ length: 1000 }, (_, index) => `line ${index}: ordinary Markdown content`).join("\n"),
			comments,
		});
		const representativePr = pullRequest({
			body: representativeIssue.body,
			comments,
			sections: {
				files: Array.from({ length: 100 }, (_, index) => ({
					path: `src/file-${index}.ts`,
					status: "modified",
					additions: index + 1,
					deletions: index % 5,
					previousPath: null,
				})),
				commits: Array.from({ length: 100 }, (_, index) => ({
					sha: String(index).padStart(40, "a"),
					title: `Commit ${index} ${"detail ".repeat(5)}`,
					author: "Alice",
					createdAt: `2026-02-${String(1 + (index % 28)).padStart(2, "0")}T00:00:00Z`,
					url: `https://github.com/Acme/widget/commit/${String(index).padStart(40, "a")}`,
				})),
				checks: Array.from({ length: 100 }, (_, index) => ({
					id: `check-${index}`,
					name: `check ${index}`,
					status: "completed",
					conclusion: "success",
					url: `https://github.com/Acme/widget/actions/${index}`,
				})),
			},
		});

		for (const artifact of [representativeIssue, representativePr]) {
			const taggedBytes = Buffer.byteLength(documentText(renderForgeDocument(repository, artifact)), "utf8");
			const jsonBytes = Buffer.byteLength(JSON.stringify(artifact), "utf8");
			expect(taggedBytes).toBeLessThanOrEqual(jsonBytes);
		}
	});

	it("renders unsupported issue relationships explicitly", () => {
		const text = documentText(
			renderForgeDocument(repository, issue({ relationships: { capability: "unsupported", items: [] } })),
		);
		expect(text).toContain('<relationships capability="unsupported"/>');
	});

	it("renders unknown and unsupported review decisions explicitly", () => {
		const unknown = documentText(
			renderForgeDocument(
				repository,
				pullRequest({
					readiness: {
						...pullRequest().readiness,
						reviewDecision: { capability: "unknown", value: "future_decision" },
					},
				}),
			),
		);
		expect(unknown).toContain('review-decision-capability="unknown" review-decision="future_decision"');

		const rendered = renderForgeDocument(
			repository,
			pullRequest({
				readiness: { ...pullRequest().readiness, reviewDecision: { capability: "unsupported" } },
			}),
		);
		const text = documentText(rendered);
		expect(text).toContain('review-decision-capability="unsupported"');
		expect(text).not.toContain('review-decision="');
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
		const text = documentText(rendered);
		expect(text).toContain('review-decision-capability="supported" review-decision="approved"');
		expect(text.indexOf("<readiness ")).toBeLessThan(text.indexOf("<title>"));
		expect(text.indexOf("</body>")).toBeLessThan(text.indexOf("<files>"));
		expect(text.indexOf("<files>")).toBeLessThan(text.indexOf("<commits>"));
		expect(text.indexOf("<commits>")).toBeLessThan(text.indexOf("<checks>"));
		expect(rendered.include).toEqual(["files", "commits", "checks"]);
	});
});

describe("sliceForgeDocument", () => {
	it("returns trusted complete field and parent-conversation coverage for a full read", () => {
		const rendered = renderForgeDocument(repository, issue());
		const slice = sliceForgeDocument(rendered, {});
		expect(slice.content).toBe(documentText(rendered));
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
		expect(slice.content).toContain("<body>first\nsecond ]]> tail</body>");
		expect(slice.content).not.toContain("CDATA");
		expect(slice.content).not.toContain("forge-break");

		const bidi = sliceForgeDocument(renderForgeDocument(repository, issue({ body: "safe\u202Espoof" })), {});
		expect(bidi.content).toContain("safe&#x202E;spoof");
		expect(bidi.content).not.toContain("\u202E");
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
		const rendered = renderForgeDocument(repository, issue({ body: "x".repeat(200_000) }));
		let offset = 1;
		let slices = 0;
		while (true) {
			const slice = sliceForgeDocument(rendered, { offset, snapshot: rendered.snapshot });
			expect(slice.details.range.lines).toBeLessThanOrEqual(2000);
			expect(Buffer.byteLength(slice.content, "utf8")).toBeLessThanOrEqual(50 * 1024);
			slices++;
			if (slice.nextOffset === undefined) break;
			expect(slice.nextOffset).toBe(offset + slice.details.range.lines);
			expect(slice.content).toContain(`Use offset=${slice.nextOffset} snapshot=${rendered.snapshot} to continue.`);
			offset = slice.nextOffset;
		}
		expect(slices).toBeGreaterThan(1);
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
