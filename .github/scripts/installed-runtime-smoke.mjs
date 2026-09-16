import { createHash } from "node:crypto";
import {
	existsSync,
	lstatSync,
	mkdirSync,
	readdirSync,
	readFileSync,
	readlinkSync,
	realpathSync,
	symlinkSync,
	writeFileSync,
} from "node:fs";
import { join, relative, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const [packageRootArg, workDir] = process.argv.slice(2);
if (!packageRootArg || !workDir) {
	throw new Error("usage: installed-runtime-smoke.mjs <installed-package-root> <work-dir>");
}
const packageRoot = realpathSync(packageRootArg);

const dataRoot = join(workDir, "data");
const home = join(workDir, "home");
const agentDir = join(workDir, "agent");
const cwd = join(workDir, "cwd");
const legacyRoot = join(dataRoot, "scramjet", "mach12");
mkdirSync(join(legacyRoot, "commands"), { recursive: true });
mkdirSync(join(legacyRoot, "agents"), { recursive: true });
mkdirSync(home, { recursive: true });
mkdirSync(cwd, { recursive: true });

const staleCommand = join(legacyRoot, "commands", "mach12:issue-create.md");
writeFileSync(staleCommand, "---\ndescription: stale\n---\n# Stale issue creation\n");
const seededHash = createHash("sha256").update(readFileSync(staleCommand)).digest("hex");
writeFileSync(staleCommand, "---\ndescription: locally edited stale command\n---\n# Edited stale issue creation\n");
writeFileSync(
	join(legacyRoot, "commands", "mach12:legacy-only.md"),
	"---\ndescription: legacy only\n---\n# Legacy only\n",
);
writeFileSync(
	join(legacyRoot, "agents", "mach12:legacy-only.md"),
	"---\nname: mach12:legacy-only\ndescription: legacy only\n---\nLegacy agent.\n",
);
writeFileSync(
	join(legacyRoot, "autonomy-defaults.yaml"),
	"edges:\n  mach12:legacy-only:\n    mach12:legacy-only: chain\n",
);
writeFileSync(
	join(legacyRoot, ".seed-manifest.json"),
	`${JSON.stringify({ version: "0.43.5", files: { "commands/mach12:issue-create.md": seededHash } }, null, "\t")}\n`,
);
const outside = join(workDir, "outside.txt");
writeFileSync(outside, "outside sentinel\n");
const linked = join(legacyRoot, "linked");
symlinkSync(outside, linked);

function snapshot(root) {
	const entries = new Map();
	function visit(path) {
		const stat = lstatSync(path);
		const key = relative(root, path) || ".";
		if (stat.isSymbolicLink()) {
			entries.set(key, `link:${readlinkSync(path)}`);
			return;
		}
		if (stat.isDirectory()) {
			entries.set(key, "directory");
			for (const name of readdirSync(path).sort()) visit(join(path, name));
			return;
		}
		entries.set(key, `file:${createHash("sha256").update(readFileSync(path)).digest("hex")}`);
	}
	visit(root);
	return JSON.stringify([...entries]);
}

const legacyBefore = snapshot(legacyRoot);
process.env.HOME = home;
process.env.XDG_DATA_HOME = dataRoot;
process.env.SCRAMJET_CACHE = join(dataRoot, "scramjet");
process.env.SCRAMJET_CODING_AGENT_DIR = agentDir;

const { initScramjet } = await import(pathToFileURL(join(resolve(packageRoot), "dist", "index.js")).href);
const tools = [];
const commands = [];
const handlers = new Map();
const appended = [];
const pi = {
	registerTool(tool) {
		tools.push(tool);
	},
	registerCommand(name, spec) {
		commands.push({ name, spec });
	},
	on(event, handler) {
		const list = handlers.get(event) ?? [];
		list.push(handler);
		handlers.set(event, list);
	},
	appendEntry(customType, data) {
		appended.push({ customType, data });
	},
	invokeHarnessTool: async () => {},
	setModel: async () => true,
	getThinkingLevel: () => "high",
	setThinkingLevel: () => {},
	sendMessage: () => {},
};
initScramjet(pi);

const discoveryHandlers = handlers.get("resources_discover") ?? [];
if (discoveryHandlers.length !== 1) throw new Error(`expected one resources_discover handler, found ${discoveryHandlers.length}`);
const discovery = await discoveryHandlers[0]({ type: "resources_discover", cwd, reason: "startup" }, { hasUI: false });
const packagePrefix = `${resolve(packageRoot)}/`;
const promptPaths = discovery?.promptPaths ?? [];
if (!promptPaths.some((path) => path === join(resolve(packageRoot), "mach12", "commands", "mach12:issue-plan.md"))) {
	throw new Error("installed runtime did not expose the packaged mach12:issue-plan command");
}
if (!promptPaths.every((path) => resolve(path).startsWith(packagePrefix))) {
	throw new Error(`discovery exposed a command outside the installed package: ${JSON.stringify(promptPaths)}`);
}
if (promptPaths.some((path) => path.includes("legacy-only"))) throw new Error("legacy-only command was discovered");

const sections = [];
const event = {
	type: "before_agent_start",
	prompt: "",
	systemPrompt: "",
	systemPromptSections: [],
	systemPromptOptions: {},
};
for (const handler of handlers.get("before_agent_start") ?? []) {
	const result = await handler(event, { hasUI: false });
	if (result?.systemPromptSection) sections.push(result.systemPromptSection);
}
const agentCatalog = sections.find((section) => section.id === "scramjet:agent-catalog")?.text ?? "";
const commandCatalog = sections.find((section) => section.id === "scramjet:command-catalog")?.text ?? "";
if (!agentCatalog.includes("mach12:structural-mapper")) throw new Error("packaged structural mapper missing from agent catalog");
if (!agentCatalog.includes("scramjet:command-reviewer")) throw new Error("packaged command reviewer missing from agent catalog");
if (agentCatalog.includes("mach12:legacy-only")) throw new Error("legacy-only agent entered the agent catalog");
if (commandCatalog.includes("mach12:legacy-only")) throw new Error("legacy-only command entered the command catalog");

const subagent = tools.find((tool) => tool.name === "subagent");
if (!subagent) throw new Error("subagent tool was not registered");
const subagentResult = await subagent.execute("installed-runtime-smoke", {}, undefined, undefined, { cwd, hasUI: false });
const subagentText = subagentResult.content?.map((item) => item.text ?? "").join("\n") ?? "";
if (!subagentText.includes("mach12:structural-mapper (package)")) {
	throw new Error(`packaged structural mapper missing from subagent discovery: ${subagentText}`);
}
if (!subagentText.includes("scramjet:command-reviewer (package)")) {
	throw new Error(`packaged command reviewer missing from subagent discovery: ${subagentText}`);
}
if (subagentText.includes("mach12:legacy-only")) throw new Error("legacy-only agent entered subagent discovery");

const warnings = appended
	.filter((entry) => entry.customType === "scramjet:log" && entry.data?.level === "warn")
	.map((entry) => entry.data.message);
if (!warnings.some((message) => message.includes(legacyRoot) && message.includes("left untouched"))) {
	throw new Error(`migration guidance was not emitted: ${JSON.stringify(warnings)}`);
}
const nonMigrationWarnings = warnings.filter((message) => !message.startsWith("Ignored legacy bundled command set"));
if (nonMigrationWarnings.some((message) => message.includes("mach12:legacy-only") || message.includes("autonomy"))) {
	throw new Error(`legacy autonomy defaults were loaded: ${JSON.stringify(warnings)}`);
}
if (existsSync(join(agentDir, "agents"))) throw new Error("runtime created an agent bridge directory");
if (snapshot(legacyRoot) !== legacyBefore) throw new Error("installed runtime changed the legacy bundled tree");
if (readFileSync(outside, "utf-8") !== "outside sentinel\n") throw new Error("installed runtime followed a legacy symlink");

process.stdout.write(
	`${JSON.stringify({ promptPaths: promptPaths.length, agentCatalog: true, subagentRegistry: true, migrationWarning: true })}\n`,
);
