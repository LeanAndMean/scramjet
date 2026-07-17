import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export type DocKey = "readme" | "vision" | "command-authoring" | "logging";

export interface DocEntry {
	key: DocKey;
	path: string;
	condition: string;
}

export function packageRoot(): string {
	let dir = dirname(fileURLToPath(import.meta.url));
	while (dir !== dirname(dir)) {
		if (existsSync(join(dir, "package.json"))) return dir;
		dir = dirname(dir);
	}
	return dir;
}

const root = packageRoot();

export const DOCS_REGISTRY: readonly DocEntry[] = [
	{
		key: "readme",
		path: join(root, "README.md"),
		condition: "read only when the user asks about Scramjet itself",
	},
	{
		key: "vision",
		path: join(root, "docs", "scramjet-vision.md"),
		condition: "read only when the user asks about Scramjet itself",
	},
	{
		key: "command-authoring",
		path: join(root, "docs", "command-authoring.md"),
		condition: "read when authoring, creating, or editing commands",
	},
	{
		key: "logging",
		path: join(root, "docs", "logging.md"),
		condition: "read when retrieving details from prior sessions or diagnosing harness behavior",
	},
];

export const DOCS_BY_KEY: Record<DocKey, DocEntry> = Object.fromEntries(DOCS_REGISTRY.map((e) => [e.key, e])) as Record<
	DocKey,
	DocEntry
>;

export function getDocPath(key: DocKey): string {
	return DOCS_BY_KEY[key].path;
}
