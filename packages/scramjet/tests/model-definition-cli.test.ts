import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, it } from "vitest";

const binary = resolve(dirname(fileURLToPath(import.meta.url)), "../bin/scramjet.js");
const cleanups: Array<() => Promise<void> | void> = [];
afterEach(async () => {
	for (const cleanup of cleanups.splice(0).reverse()) await cleanup();
});

function profile(enabledModels = ["saved-unavailable/*"]) {
	const root = mkdtempSync(join(tmpdir(), "model-definition-cli-"));
	cleanups.push(() => rmSync(root, { recursive: true, force: true }));
	const agent = join(root, "agent");
	mkdirSync(agent);
	const settings = JSON.stringify({
		defaultProvider: "saved-unavailable",
		defaultModel: "saved-model",
		defaultThinkingLevel: "low",
		enabledModels,
		enableInstallTelemetry: false,
		compaction: { enabled: false },
		retry: { enabled: false, provider: { maxRetries: 0, timeoutMs: 2000 } },
	});
	const auth = JSON.stringify({ saved: { type: "api_key", key: "stored-synthetic-key" } });
	writeFileSync(join(agent, "settings.json"), settings);
	writeFileSync(join(agent, "auth.json"), auth);
	return { root, agent, settings, auth };
}
function unchanged(p: ReturnType<typeof profile>, models?: string) {
	expect(readFileSync(join(p.agent, "settings.json"), "utf8")).toBe(p.settings);
	expect(readFileSync(join(p.agent, "auth.json"), "utf8")).toBe(p.auth);
	if (models === undefined) expect(existsSync(join(p.agent, "models.json"))).toBe(false);
	else expect(readFileSync(join(p.agent, "models.json"), "utf8")).toBe(models);
}

type Call = { path: string; headers: IncomingMessage["headers"]; body: any };
async function endpoint(handler: (call: Call, response: ServerResponse) => void) {
	const calls: Call[] = [];
	const server = createServer(async (request, response) => {
		let body = "";
		for await (const chunk of request) body += chunk;
		const call = { path: request.url!, headers: request.headers, body: JSON.parse(body) };
		calls.push(call);
		handler(call, response);
	});
	await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
	cleanups.push(
		() =>
			new Promise<void>((resolve) => {
				server.close(() => resolve());
				server.closeAllConnections();
			}),
	);
	const address = server.address();
	if (!address || typeof address === "string") throw new Error("Missing fixture port");
	return { calls, baseUrl: `http://127.0.0.1:${address.port}/fixture/v1`, port: address.port };
}

function sse(response: ServerResponse, events: any[], done = false) {
	response.writeHead(200, { "content-type": "text/event-stream" });
	for (const event of events)
		response.write(`${event.type ? `event: ${event.type}\n` : ""}data: ${JSON.stringify(event)}\n\n`);
	response.end(done ? "data: [DONE]\n\n" : "");
}
function chat(response: ServerResponse, marker: string, tool = false) {
	sse(
		response,
		[
			{
				id: "fixture",
				object: "chat.completion.chunk",
				created: 0,
				choices: [
					{
						index: 0,
						delta: tool
							? {
									tool_calls: [
										{
											index: 0,
											id: "call_read",
											type: "function",
											function: { name: "read", arguments: '{"path":"fixture.txt"}' },
										},
									],
								}
							: { content: marker },
						finish_reason: tool ? "tool_calls" : "stop",
					},
				],
				usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
			},
		],
		true,
	);
}
function success(api: string, response: ServerResponse, marker: string) {
	if (api === "openai-completions") return chat(response, marker);
	if (api === "openai-responses") {
		const item = {
			type: "message",
			id: "msg_fixture",
			role: "assistant",
			status: "completed",
			content: [{ type: "output_text", text: marker, annotations: [] }],
		};
		return sse(response, [
			{ type: "response.created", response: { id: "resp_fixture" } },
			{ type: "response.output_item.added", output_index: 0, item: { ...item, content: [] } },
			{
				type: "response.content_part.added",
				item_id: item.id,
				output_index: 0,
				content_index: 0,
				part: { type: "output_text", text: "", annotations: [] },
			},
			{ type: "response.output_text.delta", item_id: item.id, output_index: 0, content_index: 0, delta: marker },
			{ type: "response.output_item.done", output_index: 0, item },
			{
				type: "response.completed",
				response: {
					id: "resp_fixture",
					status: "completed",
					usage: {
						input_tokens: 1,
						output_tokens: 1,
						total_tokens: 2,
						input_tokens_details: { cached_tokens: 0 },
					},
				},
			},
		]);
	}
	if (api === "anthropic-messages")
		return sse(response, [
			{
				type: "message_start",
				message: {
					id: "msg_fixture",
					type: "message",
					role: "assistant",
					content: [],
					usage: { input_tokens: 1, output_tokens: 0 },
				},
			},
			{ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
			{ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: marker } },
			{ type: "content_block_stop", index: 0 },
			{ type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 1 } },
			{ type: "message_stop" },
		]);
	sse(response, [
		{
			candidates: [{ content: { parts: [{ text: marker }] }, finishReason: "STOP" }],
			usageMetadata: { promptTokenCount: 1, candidatesTokenCount: 1, totalTokenCount: 2 },
		},
	]);
}
function definition(baseUrl: string, api = "openai-completions", maxTokens = 123) {
	return { api, baseUrl, id: "literal/Model:high", apiKeyEnv: "FIXTURE_KEY", contextWindow: 100000, maxTokens };
}

function child(p: ReturnType<typeof profile>, ports: number[], args: string[], extraEnv: Record<string, string> = {}) {
	// The preload restricts transport only; routing still comes exclusively from the real CLI.
	const guard = `import net from 'node:net';
	const connect = net.Socket.prototype.connect;
	net.Socket.prototype.connect = function(...args) {
		const first = Array.isArray(args[0]) ? args[0][0] : args[0];
		const options = typeof first === 'object' ? first : {port:first,host:args[1]};
		if (options.host !== '127.0.0.1' || !${JSON.stringify(ports)}.includes(Number(options.port))) throw new Error('Non-fixture network blocked');
		return connect.apply(this,args);
	};`;
	const proc = spawn(
		process.execPath,
		[
			"--import",
			`data:text/javascript,${encodeURIComponent(guard)}`,
			binary,
			"--offline",
			"--no-extensions",
			"--no-skills",
			"--no-prompt-templates",
			"--no-themes",
			"--no-context-files",
			...args,
		],
		{
			cwd: p.root,
			env: {
				PATH: dirname(process.execPath),
				HOME: p.root,
				XDG_CONFIG_HOME: join(p.root, "config"),
				XDG_DATA_HOME: join(p.root, "data"),
				SCRAMJET_CODING_AGENT_DIR: p.agent,
				PI_TELEMETRY: "0",
				FIXTURE_KEY: "synthetic-key",
				...extraEnv,
			},
			stdio: ["pipe", "pipe", "pipe"],
		},
	);
	let stdout = "",
		stderr = "",
		buffer = "";
	const records: any[] = [];
	const invalidLines: string[] = [];
	proc.stdout.setEncoding("utf8");
	proc.stderr.setEncoding("utf8");
	proc.stdout.on("data", (data: string) => {
		stdout += data;
		buffer += data;
		while (buffer.includes("\n")) {
			const newline = buffer.indexOf("\n");
			const line = buffer.slice(0, newline);
			buffer = buffer.slice(newline + 1);
			try {
				records.push(JSON.parse(line));
			} catch {
				invalidLines.push(line);
			}
		}
	});
	proc.stderr.on("data", (data: string) => {
		stderr += data;
	});
	const closed = new Promise<{ code: number | null; stdout: string; stderr: string }>((resolve, reject) => {
		proc.on("error", reject);
		proc.on("close", (code) => resolve({ code, stdout, stderr }));
	});
	const timer = setTimeout(() => proc.kill("SIGKILL"), 20000);
	void closed.then(() => clearTimeout(timer));
	cleanups.push(async () => {
		if (proc.exitCode === null) proc.kill("SIGKILL");
		await closed;
		if (args.includes("json") || args.includes("rpc")) {
			expect(invalidLines).toEqual([]);
			expect(buffer).toBe("");
		}
	});
	async function wait(predicate: (record: any) => boolean) {
		const deadline = Date.now() + 15000;
		while (Date.now() < deadline) {
			const record = records.find(predicate);
			if (record) return record;
			if (proc.exitCode !== null) throw new Error(`Child exited: ${stderr}\n${stdout}`);
			await new Promise((resolve) => setTimeout(resolve, 10));
		}
		throw new Error(`Child response timed out: ${stderr}\n${stdout}`);
	}
	let sequence = 0;
	async function rpc(type: string, fields: Record<string, unknown> = {}) {
		const id = String(++sequence);
		proc.stdin.write(`${JSON.stringify({ id, type, ...fields })}\n`);
		return wait((record) => record.type === "response" && record.id === id);
	}
	return { proc, closed, records, wait, rpc };
}
function print(
	p: ReturnType<typeof profile>,
	server: Awaited<ReturnType<typeof endpoint>>,
	d: ReturnType<typeof definition>,
	args: string[] = [],
	env: Record<string, string> = {},
) {
	const c = child(
		p,
		[server.port],
		["--print", "--model-definition", JSON.stringify(d), ...args, "fixture prompt"],
		env,
	);
	c.proc.stdin.end();
	return c;
}

describe("compiled product invocation model acceptance", () => {
	it.each(["openai-completions", "openai-responses", "anthropic-messages", "google-generative-ai"])(
		"streams %s through the supplied route",
		async (api) => {
			const p = profile();
			const server = await endpoint((_call, response) => success(api, response, `marker-${api}`));
			const d = definition(api === "anthropic-messages" ? server.baseUrl.replace(/\/v1$/, "") : server.baseUrl, api);
			const envSelected = api === "openai-responses" || api === "google-generative-ai";
			const c = child(
				p,
				[server.port],
				[
					"--mode",
					"json",
					...(envSelected
						? ["--model-definition-env", "FIXTURE_DEFINITION"]
						: ["--model-definition", JSON.stringify(d)]),
					"fixture prompt",
				],
				{
					FIXTURE_DEFINITION: JSON.stringify(d),
					GOOGLE_GENAI_USE_VERTEXAI: "true",
					GOOGLE_GENAI_USE_ENTERPRISE: "true",
				},
			);
			c.proc.stdin.end();
			const result = await c.closed;
			expect(result.code, result.stderr).toBe(0);
			expect(
				c.records.find((r) => r.type === "message_end" && r.message.role === "assistant")?.message,
			).toMatchObject({ stopReason: "stop", model: d.id, content: [{ type: "text", text: `marker-${api}` }] });
			expect(server.calls).toHaveLength(1);
			const call = server.calls[0];
			if (api === "google-generative-ai") {
				expect(call.path).toBe(`/fixture/v1/models/${d.id}:streamGenerateContent?alt=sse`);
				expect(call.headers["x-goog-api-key"]).toBe("synthetic-key");
				expect(call.body.generationConfig.maxOutputTokens).toBe(d.maxTokens);
			} else {
				expect(call.path).toBe(
					`/fixture/v1/${api === "anthropic-messages" ? "messages" : api === "openai-responses" ? "responses" : "chat/completions"}`,
				);
				expect(call.body.model).toBe(d.id);
				expect(call.headers[api === "anthropic-messages" ? "x-api-key" : "authorization"]).toBe(
					api === "anthropic-messages" ? "synthetic-key" : "Bearer synthetic-key",
				);
				expect(
					call.body[
						api === "anthropic-messages"
							? "max_tokens"
							: api === "openai-responses"
								? "max_output_tokens"
								: "max_completion_tokens"
					],
				).toBe(d.maxTokens);
			}
			unchanged(p);
		},
		30000,
	);

	it("executes read and replays its matching result through the same Chat route", async () => {
		const p = profile();
		writeFileSync(join(p.root, "fixture.txt"), "read-result-marker");
		const server = await endpoint((_call, response) =>
			chat(response, "after-tool-marker", server.calls.length === 1),
		);
		const result = await print(p, server, definition(server.baseUrl), ["--tools", "read"]).closed;
		expect(result.code, result.stderr).toBe(0);
		expect(result.stdout).toContain("after-tool-marker");
		expect(server.calls).toHaveLength(2);
		for (const call of server.calls) {
			expect(call.body.model).toBe("literal/Model:high");
			expect(call.headers.authorization).toBe("Bearer synthetic-key");
		}
		expect(server.calls[1].body.messages).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					role: "assistant",
					tool_calls: [
						expect.objectContaining({ id: "call_read", function: expect.objectContaining({ name: "read" }) }),
					],
				}),
				expect.objectContaining({
					role: "tool",
					tool_call_id: "call_read",
					content: expect.stringContaining("read-result-marker"),
				}),
			]),
		);
		unchanged(p);
	}, 30000);

	it("isolates overlapping same-ID processes and leaves existing configuration unchanged", async () => {
		const p = profile();
		const pending: Array<() => void> = [];
		function barrier(marker: string) {
			return (_call: Call, response: ServerResponse) => {
				pending.push(() => chat(response, marker));
				if (pending.length === 2) for (const release of pending) release();
			};
		}
		const a = await endpoint(barrier("marker-a")),
			b = await endpoint(barrier("marker-b"));
		const models = JSON.stringify({ providers: {} });
		writeFileSync(join(p.agent, "models.json"), models);
		const children = [a, b].map((server, index) => {
			const c = child(
				p,
				[a.port, b.port],
				[
					"--mode",
					"json",
					"--model-definition",
					JSON.stringify(definition(server.baseUrl, "openai-completions", 123 + index)),
					`input-${index}`,
				],
				{ FIXTURE_KEY: `key-${index}` },
			);
			c.proc.stdin.end();
			return c;
		});
		const results = await Promise.all(children.map((c) => c.closed));
		expect(pending).toHaveLength(2);
		const providers = children.map(
			(c) => c.records.find((r) => r.type === "message_end" && r.message.role === "assistant")?.message.provider,
		);
		expect(providers[0]).toMatch(/^invocation-/);
		expect(providers[1]).toMatch(/^invocation-/);
		expect(providers[0]).not.toBe(providers[1]);
		[a, b].forEach((server, index) => {
			expect(results[index].code, results[index].stderr).toBe(0);
			expect(results[index].stdout).toContain(index === 0 ? "marker-a" : "marker-b");
			expect(results[index].stdout).not.toContain(index === 0 ? "marker-b" : "marker-a");
			expect(server.calls).toHaveLength(1);
			expect(server.calls[0].headers.authorization).toBe(`Bearer key-${index}`);
			expect(server.calls[0].body.max_completion_tokens).toBe(123 + index);
			expect(JSON.stringify(server.calls[0].body)).toContain(`input-${index}`);
			expect(JSON.stringify(server.calls[0].body)).not.toContain(`input-${1 - index}`);
		});
		unchanged(p, models);
	}, 30000);

	it("RPC exposes secret-free models, mutates live state and replaces sessions without saving defaults", async () => {
		const p = profile(["alternate/*"]);
		const server = await endpoint((_call, response) => chat(response, "rpc-marker"));
		const alternate = await endpoint((_call, response) => chat(response, "alternate-marker"));
		const d = definition(server.baseUrl);
		const models = JSON.stringify({
			providers: {
				alternate: {
					api: d.api,
					baseUrl: alternate.baseUrl,
					apiKey: "ALTERNATE_KEY",
					models: [{ id: "alternate", reasoning: true, contextWindow: d.contextWindow, maxTokens: 234 }],
				},
			},
		});
		writeFileSync(join(p.agent, "models.json"), models);
		const c = child(p, [server.port, alternate.port], ["--mode", "rpc", "--model-definition", JSON.stringify(d)], {
			ALTERNATE_KEY: "alternate-synthetic-key",
		});
		const state = await c.rpc("get_state");
		expect(state.success).toBe(true);
		const model = state.data.model;
		expect(model).toMatchObject({
			id: d.id,
			baseUrl: d.baseUrl,
			contextWindow: d.contextWindow,
			maxTokens: d.maxTokens,
		});
		expect(model.provider).toMatch(/^invocation-/);
		const catalog = await c.rpc("get_available_models");
		expect(catalog.data.models).toContainEqual(model);
		expect(JSON.stringify([state, catalog])).not.toContain("synthetic-key");
		expect(JSON.stringify(model)).not.toContain("FIXTURE_KEY");
		expect((await c.rpc("prompt", { message: "rpc initial" })).success).toBe(true);
		await c.wait((r) => r.type === "agent_end");
		const saved = (await c.rpc("get_state")).data.sessionFile;
		expect((await c.rpc("set_model", { provider: "alternate", modelId: "alternate" })).success).toBe(true);
		expect((await c.rpc("set_thinking_level", { level: "high" })).success).toBe(true);
		expect((await c.rpc("new_session")).success).toBe(true);
		expect((await c.rpc("get_state")).data).toMatchObject({ model: { id: "alternate" }, thinkingLevel: "high" });
		expect((await c.rpc("cycle_model")).data.model.id).toBe(d.id);
		expect((await c.rpc("set_model", { provider: "missing", modelId: "missing" })).success).toBe(false);
		expect((await c.rpc("switch_session", { sessionPath: saved })).success).toBe(true);
		expect((await c.rpc("get_state")).data.model).toEqual(model);
		expect((await c.rpc("get_available_models")).data.models).toContainEqual(model);
		const ends = c.records.filter((r) => r.type === "agent_end").length;
		await c.rpc("prompt", { message: "rpc after switch" });
		await c.wait((r) => r.type === "agent_end" && c.records.filter((e) => e.type === "agent_end").length > ends);
		c.proc.stdin.end();
		expect((await c.closed).code).toBe(0);
		expect(server.calls).toHaveLength(2);
		for (const call of server.calls) {
			expect(call.headers.authorization).toBe("Bearer synthetic-key");
			expect(call.body.model).toBe(d.id);
			expect(call.body.max_completion_tokens).toBe(d.maxTokens);
		}
		unchanged(p, models);
		expect(alternate.calls).toHaveLength(0);
		const reopened = child(p, [server.port, alternate.port], ["--mode", "rpc", "--session", saved], {
			FIXTURE_DEFINITION: JSON.stringify(d),
			ALTERNATE_KEY: "alternate-synthetic-key",
		});
		const restored = await reopened.rpc("get_state");
		expect(restored.data.model.provider).not.toBe(model.provider);
		expect(restored.data.model.id).not.toBe(d.id);
		expect(restored.data.model.baseUrl).toBe(alternate.baseUrl);
		expect(
			(await reopened.rpc("get_available_models")).data.models.some(
				(m: any) => m.provider === model.provider || m.baseUrl === d.baseUrl,
			),
		).toBe(false);
		reopened.proc.stdin.end();
		const reopenedResult = await reopened.closed;
		expect(reopenedResult.code).toBe(0);
		expect(reopenedResult.stderr).toContain("Could not restore model");
		expect(server.calls).toHaveLength(2);
		unchanged(p, models);
	}, 30000);

	it.each(["malformed", "missing-definition-env", "missing-key"])(
		"rejects %s before any request or profile mutation",
		async (kind) => {
			const p = profile();
			const server = await endpoint((_call, response) => chat(response, "must-not-run"));
			const args =
				kind === "missing-definition-env"
					? ["--model-definition-env", "MISSING"]
					: [
							"--model-definition",
							kind === "malformed"
								? '{"secret":"private-marker"'
								: JSON.stringify({ ...definition(server.baseUrl), apiKeyEnv: "MISSING" }),
						];
			const c = child(p, [server.port], ["--print", ...args, "prompt"]);
			c.proc.stdin.end();
			const result = await c.closed;
			expect(result.code).not.toBe(0);
			expect(result.stderr).toContain(kind === "malformed" ? "strict JSON" : "nonblank value");
			expect(result.stderr).not.toContain("private-marker");
			expect(server.calls).toHaveLength(0);
			unchanged(p);
		},
		30000,
	);

	it.each([
		[401, "text"],
		[404, "json"],
		[401, "rpc"],
		["eof", "json"],
	] as const)(
		"surfaces %s failures in %s without route fallback",
		async (failure, mode) => {
			const p = profile();
			const server = await endpoint((_call, response) => {
				if (failure === "eof") sse(response, [{ type: "response.created", response: { id: "incomplete" } }]);
				else {
					response.writeHead(failure, { "content-type": "application/json" });
					response.end(JSON.stringify({ error: { message: "fixture-rejection", type: "invalid_request_error" } }));
				}
			});
			const alternate = await endpoint((_call, response) => chat(response, "must-not-fallback"));
			const models = JSON.stringify({
				providers: {
					alternate: {
						api: "openai-completions",
						baseUrl: alternate.baseUrl,
						apiKey: "FIXTURE_KEY",
						models: [{ id: "alternate" }],
					},
				},
			});
			writeFileSync(join(p.agent, "models.json"), models);
			const d = definition(server.baseUrl, failure === "eof" ? "openai-responses" : "openai-completions");
			const c = child(
				p,
				[server.port, alternate.port],
				[
					mode === "text" ? "--print" : "--mode",
					...(mode === "text" ? [] : [mode]),
					"--model-definition",
					JSON.stringify(d),
					...(mode === "rpc" ? [] : ["failure prompt"]),
				],
			);
			if (mode === "rpc") {
				expect((await c.rpc("prompt", { message: "failure prompt" })).success).toBe(true);
				await c.wait((r) => r.type === "agent_end");
			}
			c.proc.stdin.end();
			const result = await c.closed;
			if (mode === "text") {
				expect(result.code).toBe(1);
				expect(result.stderr).toContain("401");
			} else {
				expect(result.code, result.stderr).toBe(0);
				const assistant = c.records.find(
					(r) => r.type === "message_end" && r.message.role === "assistant",
				)?.message;
				expect(assistant).toMatchObject({ stopReason: "error", model: d.id });
				expect(assistant.errorMessage).toMatch(
					failure === "eof" ? /terminal|incomplete|ended/i : /fixture-rejection/,
				);
			}
			expect(server.calls).toHaveLength(1);
			expect(alternate.calls).toHaveLength(0);
			unchanged(p, models);
		},
		30000,
	);
});
