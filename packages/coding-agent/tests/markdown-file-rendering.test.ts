import { type Component, Container, getRenderedCopy, Markdown } from "@leanandmean/tui";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ToolRenderContext } from "../src/core/extensions/types.js";
import { createReadToolDefinition } from "../src/core/tools/read.js";
import { DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, truncateHead } from "../src/core/tools/truncate.js";
import { initTheme, theme } from "../src/modes/interactive/theme/theme.js";
import { stripAnsi } from "../src/utils/ansi.js";

const cwd = "/tmp/markdown-file-rendering";
function context(overrides: Partial<ToolRenderContext> = {}): ToolRenderContext {
	return {
		args: { path: "document.md" },
		toolCallId: "file",
		invalidate: () => {},
		lastComponent: undefined,
		state: {},
		cwd,
		executionStarted: true,
		argsComplete: true,
		isPartial: false,
		expanded: true,
		showImages: false,
		isError: false,
		markdownToolDisplay: "rendered",
		...overrides,
	};
}
function plain(component: Component, width = 90): string {
	return component.render(width).map(stripAnsi).join("\n");
}
function documents(component: Component): Markdown[] {
	return component instanceof Markdown
		? [component]
		: component instanceof Container
			? component.children.flatMap(documents)
			: [];
}
async function read(source: string, args: { path: string; offset?: number; limit?: number } = { path: "document.md" }) {
	const definition = createReadToolDefinition(cwd, {
		operations: {
			access: async () => {},
			readFile: async () => Buffer.from(source),
		},
	});
	return { definition, result: await definition.execute("file", args) };
}
function readCard(
	definition: ReturnType<typeof createReadToolDefinition>,
	result: Awaited<ReturnType<typeof definition.execute>>,
	ctx = context(),
) {
	return definition.renderResult!(result, { expanded: ctx.expanded, isPartial: ctx.isPartial }, theme, ctx);
}
const document =
	"# Heading\n\n**emphasis** and *italic*\n\n- first\n- second\n\n```ts\nconst answer = 42;\n```\n<!-- inspection comment -->";

beforeEach(() => initTheme("pi-dark"));

describe("read Markdown provenance and presentation", () => {
	it.each(["document.md", "document.MD", "document.MarkDown"])(
		"renders eligible %s with document components",
		async (path) => {
			const { definition, result } = await read(document, { path });
			const card = readCard(definition, result, context({ args: { path } }));
			expect(result.details?.sourceTextLength).toBe(document.length);
			expect(result.content).toEqual([{ type: "text", text: document }]);
			expect(documents(card)).toHaveLength(1);
			expect(plain(card)).toContain("emphasis and italic");
			expect(plain(card)).toContain("- first");
			expect(plain(card)).not.toContain("inspection comment");
		},
	);
	it.each([undefined, "source"] as const)("keeps comments and syntax in %s mode", async (markdownToolDisplay) => {
		const { definition, result } = await read(document);
		const card = readCard(definition, result, context({ markdownToolDisplay }));
		expect(documents(card)).toHaveLength(0);
		expect(plain(card)).toContain("**emphasis**");
		expect(plain(card)).toContain("<!-- inspection comment -->");
	});
	it.each(["source", "rendered"] as const)("suppresses compact reads in %s mode", async (markdownToolDisplay) => {
		const { definition, result } = await read(document, { path: "AGENTS.md" });
		expect(
			plain(
				readCard(
					definition,
					result,
					context({ args: { path: "AGENTS.md" }, expanded: false, markdownToolDisplay }),
				),
			),
		).toBe("");
		expect(
			plain(readCard(definition, result, context({ args: { path: "AGENTS.md" }, markdownToolDisplay }))),
		).toContain("Heading");
	});
	it("selects ten source lines before parsing and reuses unchanged document components", async () => {
		const source = Array.from({ length: 15 }, (_, i) => `**line${i + 1}**`).join("\n");
		const { definition, result } = await read(source);
		const ctx = context({ expanded: false });
		const card = readCard(definition, result, ctx);
		expect(plain(card)).toContain("5 more source lines");
		expect(plain(card)).not.toContain("line11");
		const md = documents(card)[0];
		const setText = vi.spyOn(md, "setText");
		ctx.lastComponent = card;
		expect(readCard(definition, result, ctx)).toBe(card);
		expect(documents(card)[0]).toBe(md);
		expect(setText).not.toHaveBeenCalled();
		ctx.expanded = true;
		expect(plain(readCard(definition, result, ctx))).toContain("line15");
	});
	it.each(["```md", "<!-- unfinished"])("keeps limit guidance outside unfinished %s", async (prefix) => {
		const source = `${prefix}\n😀\r\u001b[31mtext\nlast`;
		const { definition, result } = await read(source, { path: "document.md", limit: 2 });
		expect(result.details?.sourceTextLength).toBe(`${prefix}\n😀\r\u001b[31mtext`.length);
		expect(result.content[0]).toEqual({
			type: "text",
			text: `${prefix}\n😀\r\u001b[31mtext\n\n[1 more lines in file. Use offset=3 to continue.]`,
		});
		expect(plain(readCard(definition, result))).toContain("Use offset=3 to continue.");
	});
	it("renders offset fragments as supplied and preserves literal notice-like file prose", async () => {
		const source = "ignored\n**fragment**\n[Showing lines 1-2. Use offset=3 to continue.]";
		const { definition, result } = await read(source, { path: "document.md", offset: 2 });
		expect(result.details?.sourceTextLength).toBe(source.slice(8).length);
		expect(plain(readCard(definition, result))).toContain("fragment");
		expect(plain(readCard(definition, result))).toContain("[Showing lines 1-2.");
	});
	it.each(["lines", "bytes"])("separates actual %s truncation and continuation guidance", async (kind) => {
		const source =
			kind === "lines"
				? `<!--\n${"hidden\n".repeat(DEFAULT_MAX_LINES)}`
				: `<!--\n${`${"x".repeat(1024)}\n`.repeat(60)}`;
		const { definition, result } = await read(source);
		expect(result.details?.truncation?.truncatedBy).toBe(kind);
		expect(result.details?.sourceTextLength).toBe(result.details?.truncation?.content.length);
		const card = readCard(definition, result, context({ expanded: false }));
		expect(plain(card)).toContain("Use offset=");
		expect(plain(card)).toContain("Truncated:");
	});
	it("uses a zero document boundary for diagnostic-only oversized first lines", async () => {
		const { definition, result } = await read("x".repeat(DEFAULT_MAX_BYTES + 1));
		expect(result.details?.sourceTextLength).toBe(0);
		const card = readCard(definition, result);
		expect(documents(card)).toHaveLength(0);
		expect(plain(card)).toContain("Use bash:");
	});
	it.each([undefined, -1, 1.5, 10000, NaN, "5"])(
		"labels ambiguous/invalid provenance %s as Source without dropping text",
		async (boundary) => {
			const { definition } = await read("");
			const result = {
				content: [{ type: "text" as const, text: "**literal**\n<!-- comment -->" }],
				details: { sourceTextLength: boundary },
			};
			const card = readCard(definition, result as never);
			expect(documents(card)).toHaveLength(0);
			expect(plain(card)).toContain("Source");
			expect(plain(card)).toContain("**literal**");
			expect(plain(card)).toContain("<!-- comment -->");
		},
	);
	it("accepts only verified legacy truncation prefixes", async () => {
		const { definition } = await read("");
		const truncation = truncateHead("**one**\ntwo", { maxLines: 1 });
		const result = {
			content: [{ type: "text" as const, text: `${truncation.content}\n\n[guidance]` }],
			details: { truncation },
		};
		expect(documents(readCard(definition, result))).toHaveLength(1);
		result.content[0].text = "different **body**";
		expect(documents(readCard(definition, result))).toHaveLength(0);
		expect(plain(readCard(definition, result))).toContain("different **body**");
	});
	it("does not guess legacy provenance when explicit metadata is invalid", async () => {
		const { definition } = await read("");
		const truncation = truncateHead("**one**\ntwo", { maxLines: 1 });
		const result = {
			content: [{ type: "text" as const, text: `${truncation.content}\n\n[guidance]` }],
			details: { truncation, sourceTextLength: -1 },
		};
		expect(documents(readCard(definition, result))).toHaveLength(0);
	});
	it("preserves plain non-Markdown, error and image presentations", async () => {
		const { definition, result } = await read(document);
		for (const ctx of [context({ isError: true }), context({ args: { path: "document.txt" } })]) {
			expect(documents(readCard(definition, result, ctx))).toHaveLength(0);
		}
		const image = {
			content: [
				{ type: "text" as const, text: "Read image file [image/png]" },
				{ type: "image" as const, data: "", mimeType: "image/png" },
			],
			details: undefined,
		};
		expect(plain(readCard(definition, image))).toContain("image/png");
		expect(documents(readCard(definition, image))).toHaveLength(0);
		const disguised = createReadToolDefinition(cwd, {
			autoResizeImages: false,
			operations: {
				access: async () => {},
				readFile: async () => Buffer.from("image"),
				detectImageMimeType: async () => "image/png",
			},
		});
		const imageResult = await disguised.execute("file", { path: "image.md" });
		expect(documents(readCard(disguised, imageResult))).toHaveLength(0);
	});
	it("retains copy provenance through document/chrome composition and narrow reflow", async () => {
		const { definition, result } = await read(
			"```txt\n  indented content\n```\n\nlong words wrapping across narrow columns",
		);
		const card = readCard(definition, result);
		const rows = card.render(18);
		expect(getRenderedCopy(rows).some((row) => row?.after !== undefined)).toBe(true);
		expect(getRenderedCopy(rows).some((row) => row && row.start > 0)).toBe(true);
		card.invalidate();
		expect(plain(card, 80)).toContain("indented content");
	});
});

describe("read presentation safety and invalidation", () => {
	it("refreshes document and Source presentation through mode and theme changes", async () => {
		const { definition, result } = await read(document);
		const ctx = context();
		const card = readCard(definition, result, ctx);
		const oldRendered = card.render(80).join("\n");
		initTheme("pi-light");
		card.invalidate();
		ctx.lastComponent = card;
		expect(readCard(definition, result, ctx)).toBe(card);
		expect(card.render(80).join("\n")).not.toBe(oldRendered);
		ctx.markdownToolDisplay = "source";
		ctx.lastComponent = readCard(definition, result, ctx);
		expect(plain(ctx.lastComponent)).toContain("<!-- inspection comment -->");
		ctx.markdownToolDisplay = "rendered";
		expect(plain(readCard(definition, result, ctx))).not.toContain("inspection comment");
	});
	it.each(["source", "rendered"] as const)(
		"neutralizes file-controlled terminal sequences in %s mode",
		async (markdownToolDisplay) => {
			const content = "**safe**\u001b]52;c;PAYLOAD\u0007\u001b[2J\u202eevil\u0000";
			const { definition, result } = await read(content);
			const rows = readCard(definition, result, context({ markdownToolDisplay })).render(90).join("\n");
			expect(rows).not.toContain("\u001b]52");
			expect(rows).not.toContain("\u001b[2J");
			expect(rows).not.toContain("\u202e");
			expect(rows).not.toContain("\u0000");
		},
	);
});
