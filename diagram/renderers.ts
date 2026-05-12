/**
 * Diagram renderer backends. Detects installed renderers and shells out to them.
 */

import { execSync, execFileSync } from "node:child_process";
import { writeFileSync, readFileSync, mkdtempSync, rmSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

export type DiagramFormat = "mermaid" | "graphviz" | "plantuml";

interface RendererInfo {
	available: boolean;
	command: string;
}

const rendererCache = new Map<DiagramFormat, RendererInfo>();

function commandExists(cmd: string): boolean {
	try {
		execSync(`which ${cmd}`, { stdio: "ignore" });
		return true;
	} catch {
		return false;
	}
}

export function detectRenderers(): Record<DiagramFormat, RendererInfo> {
	if (rendererCache.size === 0) {
		rendererCache.set("mermaid", {
			available: commandExists("mmdc"),
			command: "mmdc",
		});
		rendererCache.set("graphviz", {
			available: commandExists("dot"),
			command: "dot",
		});
		rendererCache.set("plantuml", {
			available: commandExists("plantuml"),
			command: "plantuml",
		});
	}

	return Object.fromEntries(rendererCache) as Record<DiagramFormat, RendererInfo>;
}

export function renderDiagram(source: string, format: DiagramFormat, signal?: AbortSignal): Buffer {
	const renderers = detectRenderers();
	const renderer = renderers[format];

	if (!renderer.available) {
		const installHints: Record<DiagramFormat, string> = {
			mermaid: "npm install -g @mermaid-js/mermaid-cli",
			graphviz: "apt install graphviz  (or brew install graphviz)",
			plantuml: "apt install plantuml  (or brew install plantuml)",
		};
		throw new Error(`${format} renderer (${renderer.command}) not found. Install with: ${installHints[format]}`);
	}

	const tmp = mkdtempSync(join(tmpdir(), "scramjet-diagram-"));
	try {
		const extensions: Record<DiagramFormat, string> = { mermaid: ".mmd", graphviz: ".dot", plantuml: ".puml" };
		const inputFile = join(tmp, `input${extensions[format]}`);
		const outputFile = join(tmp, "output.png");

		writeFileSync(inputFile, source);

		signal?.throwIfAborted();

		switch (format) {
			case "mermaid":
				execFileSync("mmdc", ["-i", inputFile, "-o", outputFile, "-b", "transparent"], {
					timeout: 30_000,
					stdio: "ignore",
				});
				break;
			case "graphviz":
				execFileSync("dot", ["-Tpng", inputFile, "-o", outputFile], {
					timeout: 30_000,
					stdio: "ignore",
				});
				break;
			case "plantuml":
				execFileSync("plantuml", ["-tpng", inputFile], {
					timeout: 30_000,
					stdio: "ignore",
				});
				break;
		}

		return readFileSync(outputFile);
	} finally {
		rmSync(tmp, { recursive: true, force: true });
	}
}
