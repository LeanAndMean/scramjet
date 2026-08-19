import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, unlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import lockfile from "proper-lockfile";
import { afterEach, describe, expect, it } from "vitest";
import { createAgentSessionServices } from "../src/core/agent-session-services.js";
import {
	type OutputThroughputHistoryDiagnostic,
	OutputThroughputHistoryStore,
} from "../src/core/output-throughput-history.js";

const temporaryDirectories: string[] = [];
const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");

async function temporaryPath(): Promise<string> {
	const directory = await mkdtemp(join(tmpdir(), "output-throughput-history-"));
	temporaryDirectories.push(directory);
	return join(directory, "output-throughput-history.json");
}

function sample(model: string, observedAt: number) {
	return {
		provider: "test-provider",
		model,
		outputTokens: observedAt + 1,
		durationMs: 1000,
		observedAt,
	};
}

afterEach(async () => {
	await Promise.all(
		temporaryDirectories.splice(0).map((directory) => rm(directory, { recursive: true, force: true })),
	);
});

describe("output-throughput history construction", () => {
	it("derives the history path from the resolved custom profile", async () => {
		const path = await temporaryPath();
		const agentDir = join(path, "profile");
		const services = await createAgentSessionServices({ cwd: dirname(path), agentDir });

		expect(services.outputThroughputHistoryPath).toBe(join(agentDir, "output-throughput-history.json"));
	});
});

describe("OutputThroughputHistoryStore", () => {
	it("loads missing history as empty and round-trips valid samples", async () => {
		const path = await temporaryPath();
		const store = new OutputThroughputHistoryStore(path);

		expect((await store.refresh()).samples("test-provider", "model")).toEqual([]);
		store.submit(sample("model", 1));
		await store.refresh();

		const reloaded = new OutputThroughputHistoryStore(path);
		expect((await reloaded.refresh()).samples("test-provider", "model")).toEqual([sample("model", 1)]);
		expect(JSON.parse(await readFile(path, "utf8"))).toEqual({ version: 1, samples: [sample("model", 1)] });
	});

	it("merges independent writers and bounds each requested model to its latest 20 samples", async () => {
		const path = await temporaryPath();
		const first = new OutputThroughputHistoryStore(path);
		const second = new OutputThroughputHistoryStore(path);

		for (let index = 0; index < 22; index++) first.submit(sample("one", index));
		second.submit(sample("two", 100));
		await second.refresh();

		const history = await first.refresh();
		expect(history.samples("test-provider", "one").map(({ observedAt }) => observedAt)).toEqual(
			Array.from({ length: 20 }, (_, index) => index + 2),
		);
		expect(history.samples("test-provider", "two")).toEqual([sample("two", 100)]);
	});

	it("merges writers from separate processes", async () => {
		const path = await temporaryPath();
		const scriptPath = join(dirname(path), "writer.ts");
		const modulePath = join(repositoryRoot, "packages/coding-agent/src/core/output-throughput-history.ts");
		await writeFile(
			scriptPath,
			`import { writeFile } from "node:fs/promises";\n` +
				`import { OutputThroughputHistoryStore } from ${JSON.stringify(modulePath)};\n` +
				`const [path, model, observedAt, readyPath] = process.argv.slice(2);\n` +
				`await writeFile(readyPath!, "ready");\n` +
				`const store = new OutputThroughputHistoryStore(path!);\n` +
				`store.submit({ provider: "test-provider", model: model!, outputTokens: Number(observedAt) + 1, durationMs: 1000, observedAt: Number(observedAt) });\n` +
				`await store.refresh();\n`,
		);
		const viteNode = join(repositoryRoot, "node_modules/vite-node/vite-node.mjs");
		const anchorPath = join(dirname(path), "output-throughput-history.lock");
		const readyPaths = [join(dirname(path), "one.ready"), join(dirname(path), "two.ready")];
		await writeFile(anchorPath, "");
		const release = await lockfile.lock(anchorPath, { realpath: false });
		const writers = [
			execFileAsync(process.execPath, [viteNode, scriptPath, path, "one", "1", readyPaths[0] ?? ""]),
			execFileAsync(process.execPath, [viteNode, scriptPath, path, "two", "2", readyPaths[1] ?? ""]),
		];
		while (true) {
			const ready = await Promise.all(
				readyPaths.map((readyPath) =>
					readFile(readyPath)
						.then(() => true)
						.catch(() => false),
				),
			);
			if (ready.every(Boolean)) break;
			await new Promise((resolve) => setTimeout(resolve, 10));
		}
		await new Promise((resolve) => setTimeout(resolve, 50));
		await release();
		await Promise.all(writers);

		const history = await new OutputThroughputHistoryStore(path).refresh();
		expect(history.samples("test-provider", "one")).toEqual([sample("one", 1)]);
		expect(history.samples("test-provider", "two")).toEqual([sample("two", 2)]);
	});

	it("keeps written documents within the global schema bound", async () => {
		const path = await temporaryPath();
		const samples = Array.from({ length: 20000 }, (_, index) => sample(`model-${index}`, index));
		await writeFile(path, JSON.stringify({ version: 1, samples }));
		const store = new OutputThroughputHistoryStore(path);

		store.submit(sample("newest", 20000));
		await store.refresh();

		const document = JSON.parse(await readFile(path, "utf8"));
		expect(document.samples).toHaveLength(20000);
		expect(document.samples.at(-1)).toEqual(sample("newest", 20000));
		expect((await new OutputThroughputHistoryStore(path).refresh()).samples("test-provider", "newest")).toEqual([
			sample("newest", 20000),
		]);
	});

	it("preserves malformed and unsupported files and the last valid snapshot", async () => {
		const path = await temporaryPath();
		const diagnostics: OutputThroughputHistoryDiagnostic[] = [];
		const store = new OutputThroughputHistoryStore(path, (diagnostic) => diagnostics.push(diagnostic));
		store.submit(sample("model", 1));
		await store.refresh();

		for (const invalid of ["not json", JSON.stringify({ version: 2, samples: [] })]) {
			await writeFile(path, invalid);
			expect((await store.refresh()).samples("test-provider", "model")).toEqual([sample("model", 1)]);
			expect(await readFile(path, "utf8")).toBe(invalid);
			store.submit(sample("model", 2));
			await store.refresh();
			expect(await readFile(path, "utf8")).toBe(invalid);
		}
		expect(diagnostics.every(({ message }) => message.includes("preserved"))).toBe(true);
	});

	it("normalizes accepted samples before re-persisting them", async () => {
		const path = await temporaryPath();
		await writeFile(path, JSON.stringify({ version: 1, samples: [{ ...sample("model", 1), unexpected: true }] }));
		const store = new OutputThroughputHistoryStore(path);

		await store.refresh();
		store.submit(sample("other", 2));
		await store.refresh();

		expect(JSON.parse(await readFile(path, "utf8"))).toEqual({
			version: 1,
			samples: [sample("model", 1), sample("other", 2)],
		});
	});

	it("reports canonical-path failures without rejecting", async () => {
		const path = await temporaryPath();
		await writeFile(path, "not a directory");
		const invalidPath = join(path, "history.json");
		const diagnostics: OutputThroughputHistoryDiagnostic[] = [];
		const store = new OutputThroughputHistoryStore(invalidPath, (diagnostic) => diagnostics.push(diagnostic));

		store.submit(sample("model", 1));
		await expect(store.refresh()).resolves.toBeDefined();
		expect(diagnostics).toHaveLength(1);
		expect(diagnostics[0]?.message).toContain("preserved");

		await unlink(path);
		store.submit(sample("model", 1));
		await store.refresh();
		expect(store.snapshot().samples("test-provider", "model")).toEqual([sample("model", 1)]);
	});

	it("rejects invalid boundary values and deduplicates equivalent diagnostics", async () => {
		const path = await temporaryPath();
		const diagnostics: OutputThroughputHistoryDiagnostic[] = [];
		await writeFile(path, JSON.stringify({ version: 1, samples: [{ ...sample("model", 1), provider: "" }] }));
		const store = new OutputThroughputHistoryStore(path, (diagnostic) => diagnostics.push(diagnostic));

		await store.refresh();
		await store.refresh();

		expect(store.snapshot().samples("test-provider", "model")).toEqual([]);
		expect(diagnostics).toHaveLength(1);
	});
});
