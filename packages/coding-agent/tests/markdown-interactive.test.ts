import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { stripVTControlCharacters } from "node:util";
import type { AssistantMessage } from "@leanandmean/ai";
import { Text } from "@leanandmean/tui";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createToolHtmlRenderer } from "../src/core/export-html/tool-renderer.js";
import type { ExtensionFactory } from "../src/core/extensions/index.js";
import { SettingsManager } from "../src/core/settings-manager.js";
import { createReadToolDefinition } from "../src/core/tools/read.js";
import { ToolExecutionComponent } from "../src/modes/interactive/components/tool-execution.js";
import { theme } from "../src/modes/interactive/theme/theme.js";
import { createProductionInteractiveHarness } from "./helpers/interactive-harness.js";

vi.mock("../src/utils/tools-manager.js", () => ({ ensureTool: vi.fn(async () => undefined) }));

const harnesses: Awaited<ReturnType<typeof createProductionInteractiveHarness>>[] = [];
const document = `# Heading\n\n**Emphasis**\n\n- Item\n<!-- hidden comment -->\n${"body\n".repeat(12)}`;
const read = createReadToolDefinition("/tmp", {
	operations: { access: async () => {}, readFile: async () => Buffer.from(document) },
});

function storage(project: object = {}, saveError = new Error("synthetic save failure\x1b[2J")) {
	let durable = JSON.stringify({ theme: "pi-dark", quietStartup: true, dockEditor: false });
	let failure = false;
	const manager = SettingsManager.fromStorage({
		withLock(scope, update) {
			const next = update(scope === "global" ? durable : JSON.stringify(project));
			if (scope === "global" && next !== undefined) {
				if (failure) throw saveError;
				durable = next;
			}
		},
	});
	return {
		manager,
		saved: () => JSON.parse(durable),
		fail: (value: boolean) => {
			failure = value;
		},
	};
}

async function setup(manager = storage().manager, extension?: ExtensionFactory, viewport = true) {
	const h = await createProductionInteractiveHarness(80, 32, extension, viewport, manager);
	harnesses.push(h);
	h.extensionUI.setFooter(() => new Text("FOOTER", 0, 0));
	return h;
}

type Harness = Awaited<ReturnType<typeof setup>>;
function call(id: string): AssistantMessage {
	return {
		role: "assistant",
		content: [{ type: "toolCall", id, name: "read", arguments: { path: "sample.md" } }],
		api: "anthropic-messages",
		provider: "anthropic",
		model: "test",
		timestamp: Date.now(),
		stopReason: "toolUse",
		usage: {
			input: 0,
			output: 0,
			cacheRead: 0,
			cacheWrite: 0,
			totalTokens: 0,
			cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
		},
	};
}
function cards(h: Harness) {
	return [...h.internals.committedChatContainer.children, ...h.internals.chatContainer.children].filter(
		(child): child is ToolExecutionComponent => child instanceof ToolExecutionComponent,
	);
}
function output(card: ToolExecutionComponent) {
	return stripVTControlCharacters(card.render(76).join("\n"));
}
async function start(h: Harness, id: string, streamed = false) {
	if (streamed) {
		await h.emit({ type: "message_start", message: call(id) });
		await h.emit({ type: "message_update", message: call(id) });
	}
	await h.emit({ type: "tool_execution_start", toolCallId: id, toolName: "read", args: { path: "sample.md" } });
	return cards(h).at(-1)!;
}
async function finish(h: Harness, id: string) {
	await h.emit({
		type: "tool_execution_end",
		toolCallId: id,
		result: await read.execute(id, { path: "sample.md" }, undefined, undefined, undefined as never),
		isError: false,
	});
}
async function settings(h: Harness) {
	h.extensionUI.setEditorText("/settings");
	h.terminal.sendInput("\r");
	await h.frame();
	for (const char of "Markdown reads") h.terminal.sendInput(char);
	return (await h.frame()).join("\n");
}
async function toggle(h: Harness) {
	h.terminal.sendInput("\r");
	await h.session.settingsManager.flush();
	await new Promise((resolve) => setImmediate(resolve));
	return (await h.frame()).join("\n");
}

afterEach(async () => {
	for (const h of harnesses.splice(0)) await h.dispose();
	vi.restoreAllMocks();
});

describe("interactive Markdown reads", () => {
	it.each([false, true])(
		"activates rendered reads for streamed=%s and updates live/committed cards without replacing them",
		async (streamed) => {
			const store = storage();
			const h = await setup(store.manager);
			const committed = await start(h, "done", streamed);
			await finish(h, "done");
			const live = await start(h, "live");
			await h.emit({
				type: "tool_execution_update",
				toolCallId: "live",
				partialResult: await read.execute("live", { path: "sample.md" }, undefined, undefined, undefined as never),
			});
			h.internals.setToolsExpanded(true);
			expect(output(committed)).not.toContain("**Emphasis**");
			expect(output(live)).not.toContain("**Emphasis**");
			expect(await settings(h)).toContain("Markdown reads");
			expect(await toggle(h)).toContain("Source");
			expect(cards(h)).toEqual([committed, live]);
			for (const card of cards(h)) {
				expect(output(card)).toContain("**Emphasis**");
				expect(output(card)).toContain("hidden comment");
				expect(output(card)).not.toContain("more lines");
			}
			expect(store.saved().markdown.toolDisplay).toBe("source");
			expect(await toggle(h)).toContain("Rendered");
			expect(output(committed)).not.toContain("hidden comment");
			h.terminal.sendInput("\x1b");
			await h.frame();
			h.internals.setToolsExpanded(false);
			expect(output(committed)).toContain("more source lines");
			const future = await start(h, "future");
			await finish(h, "future");
			expect(output(future)).not.toContain("**Emphasis**");
		},
	);

	it.each([{}, { markdown: { codeBlockIndent: "    " } }, { markdown: { toolDisplay: "rendered" } }])(
		"shows only an exact project-leaf override: %j",
		async (project) => {
			const store = storage(project);
			const h = await setup(store.manager);
			const overridden = "markdown" in project && "toolDisplay" in project.markdown;
			expect((await settings(h)).includes("Project settings override")).toBe(overridden);
			const frame = await toggle(h);
			expect(frame).toContain(overridden ? "Rendered" : "Source");
			expect(store.manager.getMarkdownToolDisplay()).toBe(overridden ? "rendered" : "source");
			expect(store.saved().markdown.toolDisplay).toBe("source");
		},
	);

	it("shares latest save settlement, preserves long failure evidence, and clears it after recovery", async () => {
		const reason = `${"long settings reason ".repeat(20)}END_OF_SETTINGS_REASON`;
		const store = storage({}, new Error("synthetic save failure\x1b[2J\x07", { cause: { reason } }));
		const h = await setup(store.manager);
		const warnings = vi.spyOn(h.mode, "showWarning");
		await store.manager.flush();
		store.fail(true);
		await settings(h);
		h.terminal.sendInput("\r");
		h.terminal.sendInput("\r");
		await store.manager.flush();
		await vi.waitFor(async () => expect((await h.frame()).join("\n")).toContain("Changes not saved"));
		expect(warnings).toHaveBeenCalledWith(
			"Display settings not saved. Changes may be lost on restart; project overrides still apply.",
		);
		const diagnostic = warnings.mock.calls
			.map(([message]) => message)
			.find((text) => text.includes("Settings persistence"));
		expect(diagnostic).toContain("Settings persistence (global)");
		expect(diagnostic).toContain(reason);
		expect(diagnostic).not.toMatch(/[\x00-\x1f\x7f-\x9f]/);
		expect(store.saved().markdown).toBeUndefined();
		store.fail(false);
		expect(await toggle(h)).not.toContain("Changes not saved");
		expect(store.saved().markdown.toolDisplay).toBe("source");
	});

	it("uses current settings for serialized history and successful reload", async () => {
		const store = storage();
		const h = await setup(store.manager);
		const result = await read.execute("history", { path: "sample.md" }, undefined, undefined, undefined as never);
		const messages = JSON.parse(
			JSON.stringify([
				call("history"),
				{
					role: "toolResult",
					toolCallId: "history",
					toolName: "read",
					...result,
					isError: false,
					timestamp: Date.now(),
				},
			]),
		);
		for (const message of messages) h.session.sessionManager.appendMessage(message);
		h.internals.renderCurrentSessionState();
		expect(output(cards(h)[0])).not.toContain("**Emphasis**");
		await settings(h);
		await toggle(h);
		h.terminal.sendInput("\x1b");
		await h.frame();
		await h.internals.handleReloadCommand();
		expect(output(cards(h)[0])).toContain("**Emphasis**");
		expect(result.details?.sourceTextLength).toBe(document.length);
		expect(result.content[0]).toEqual({ type: "text", text: document });
	});

	it("synchronizes surviving cards if resource reload fails after settings refresh", async () => {
		const h = await setup();
		const card = await start(h, "survivor");
		await finish(h, "survivor");
		vi.spyOn(h.session, "reload").mockImplementation(async () => {
			h.session.settingsManager.setMarkdownToolDisplay("source");
			throw new Error("synthetic resource failure");
		});
		await h.internals.handleReloadCommand();
		expect(cards(h)[0]).toBe(card);
		expect(output(card)).toContain("**Emphasis**");
		expect((await h.frame()).join("\n")).toContain("synthetic resource failure");
	});

	it.each([false, true])(
		"inherits builtin read presentation but preserves custom=%s renderer authority",
		async (custom) => {
			const h = await setup(undefined, (pi) => {
				pi.registerTool({
					...read,
					renderCall: undefined,
					renderResult: custom ? () => new Text("CUSTOM **literal**", 0, 0) : undefined,
				});
			});
			const card = await start(h, "override");
			await finish(h, "override");
			expect(output(card)).toContain(custom ? "CUSTOM **literal**" : "Emphasis");
			expect(output(card)).not.toContain("**Emphasis**");
		},
	);

	it("reopens the saved profile preference and preserves malformed files on every blocked save", async () => {
		const directory = mkdtempSync(join(tmpdir(), "scramjet-markdown-reopen-"));
		const agentDir = join(directory, "profile");
		mkdirSync(agentDir);
		const file = join(agentDir, "settings.json");
		writeFileSync(file, JSON.stringify({ theme: "pi-dark", quietStartup: true }));
		try {
			const manager = SettingsManager.create(directory, agentDir);
			const h = await setup(manager);
			await settings(h);
			await toggle(h);
			const reopened = SettingsManager.create(directory, agentDir);
			expect(reopened.getMarkdownToolDisplay()).toBe("source");
			const restarted = await setup(reopened);
			const card = await start(restarted, "reopened");
			await finish(restarted, "reopened");
			expect(output(card)).toContain("**Emphasis**");
			await reopened.flush();
			writeFileSync(file, "{broken-json");
			reopened.reload();
			reopened.drainErrors();
			await settings(restarted);
			for (let attempt = 0; attempt < 2; attempt++) {
				expect(await toggle(restarted)).toContain("Changes not saved");
				expect(readFileSync(file, "utf8")).toBe("{broken-json");
			}
		} finally {
			for (const h of harnesses.splice(0)) await h.dispose();
			rmSync(directory, { recursive: true, force: true });
		}
	});

	it("keeps HTML renderer contexts in Source mode", async () => {
		await setup();
		const renderer = createToolHtmlRenderer({ getToolDefinition: () => read, theme, cwd: "/tmp" });
		renderer.renderCall("html", "read", { path: "sample.md" });
		const result = await read.execute("html", { path: "sample.md" }, undefined, undefined, undefined as never);
		const html = renderer.renderResult("html", "read", result.content, result.details, false)?.expanded;
		expect(html).toBeDefined();
		expect(html!.replace(/<[^>]*>/g, "")).toContain("**Emphasis**");
	});
});
