import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export interface DocEntry {
	key: string;
	label: string;
	path: string;
	condition: string;
}

function packageRoot(): string {
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
		label: "README",
		path: join(root, "README.md"),
		condition: "read only when the user asks about Scramjet itself",
	},
	{
		key: "vision",
		label: "Vision / design",
		path: join(root, "docs", "scramjet-vision.md"),
		condition: "read only when the user asks about Scramjet itself",
	},
	{
		key: "command-authoring",
		label: "Command authoring",
		path: join(root, "docs", "command-authoring.md"),
		condition: "read when authoring, creating, or editing commands",
	},
];

export function getDocPath(key: string): string | undefined {
	return DOCS_REGISTRY.find((e) => e.key === key)?.path;
}
