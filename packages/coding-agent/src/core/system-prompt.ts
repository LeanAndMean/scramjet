/**
 * System prompt construction and project context loading
 */

import { flattenSystemPrompt, type SystemPromptSection } from "@scramjet/ai";
import { getDocsPath, getExamplesPath, getReadmePath } from "../config.js";
import { formatSkillsForPrompt, type Skill } from "./skills.js";

export interface BuildSystemPromptOptions {
	/** Custom system prompt (replaces default). */
	customPrompt?: string;
	/** Tools to include in prompt. Default: [read, bash, edit, write] */
	selectedTools?: string[];
	/** Optional one-line tool snippets keyed by tool name. */
	toolSnippets?: Record<string, string>;
	/** Additional guideline bullets appended to the default system prompt guidelines. */
	promptGuidelines?: string[];
	/** Text to append to system prompt. */
	appendSystemPrompt?: string;
	/** Working directory. */
	cwd: string;
	/** Pre-loaded context files. */
	contextFiles?: Array<{ path: string; content: string }>;
	/** Pre-loaded skills. */
	skills?: Skill[];
}

/**
 * Build the system prompt as ordered sections.
 *
 * Each section's `text` carries its own leading separator, so flattening the
 * sections with an empty-string join reproduces the single-string prompt
 * byte-for-byte. The volatile date/cwd tail is the last section and is marked
 * `cacheRetention: "none"` so providers can exclude it from cached prefixes.
 */
export function buildSystemPromptSections(options: BuildSystemPromptOptions): SystemPromptSection[] {
	const {
		customPrompt,
		selectedTools,
		appendSystemPrompt,
		cwd,
		contextFiles: providedContextFiles,
		skills: providedSkills,
	} = options;
	const promptCwd = cwd.replace(/\\/g, "/");

	const now = new Date();
	const year = now.getFullYear();
	const month = String(now.getMonth() + 1).padStart(2, "0");
	const day = String(now.getDate()).padStart(2, "0");
	const date = `${year}-${month}-${day}`;

	const sections: SystemPromptSection[] = [];

	sections.push({ id: "core", text: customPrompt || buildDefaultCorePrompt(options) });

	if (appendSystemPrompt) {
		sections.push({ id: "append", text: `\n\n${appendSystemPrompt}` });
	}

	const contextFiles = providedContextFiles ?? [];
	if (contextFiles.length > 0) {
		let text = "\n\n# Project Context\n\n";
		text += "Project-specific instructions and guidelines:\n\n";
		for (const { path: filePath, content } of contextFiles) {
			text += `## ${filePath}\n\n${content}\n\n`;
		}
		sections.push({ id: "context-files", text });
	}

	// Skills section (only if read tool is available)
	const skills = providedSkills ?? [];
	const hasRead = !selectedTools || selectedTools.includes("read");
	if (hasRead && skills.length > 0) {
		const text = formatSkillsForPrompt(skills);
		if (text) {
			sections.push({ id: "skills", text });
		}
	}

	// Date and working directory last, excluded from cached prefixes
	sections.push({
		id: "volatile",
		text: `\nCurrent date: ${date}\nCurrent working directory: ${promptCwd}`,
		cacheRetention: "none",
	});

	return sections;
}

/** Build the system prompt with tools, guidelines, and context */
export function buildSystemPrompt(options: BuildSystemPromptOptions): string {
	return flattenSystemPrompt(buildSystemPromptSections(options));
}

/** Build the default core prompt: identity, tools list, guidelines, doc paths. */
function buildDefaultCorePrompt(options: BuildSystemPromptOptions): string {
	const { selectedTools, toolSnippets, promptGuidelines } = options;

	// Get absolute paths to documentation and examples
	const readmePath = getReadmePath();
	const docsPath = getDocsPath();
	const examplesPath = getExamplesPath();

	// Build tools list based on selected tools.
	// A tool appears in Available tools only when the caller provides a one-line snippet.
	const tools = selectedTools || ["read", "bash", "edit", "write"];
	const visibleTools = tools.filter((name) => !!toolSnippets?.[name]);
	const toolsList =
		visibleTools.length > 0 ? visibleTools.map((name) => `- ${name}: ${toolSnippets![name]}`).join("\n") : "(none)";

	// Build guidelines based on which tools are actually available
	const guidelinesList: string[] = [];
	const guidelinesSet = new Set<string>();
	const addGuideline = (guideline: string): void => {
		if (guidelinesSet.has(guideline)) {
			return;
		}
		guidelinesSet.add(guideline);
		guidelinesList.push(guideline);
	};

	const hasBash = tools.includes("bash");
	const hasGrep = tools.includes("grep");
	const hasFind = tools.includes("find");
	const hasLs = tools.includes("ls");

	// File exploration guidelines
	if (hasBash && !hasGrep && !hasFind && !hasLs) {
		addGuideline("Use bash for file operations like ls, rg, find");
	} else if (hasBash && (hasGrep || hasFind || hasLs)) {
		addGuideline("Prefer grep/find/ls tools over bash for file exploration (faster, respects .gitignore)");
	}

	for (const guideline of promptGuidelines ?? []) {
		const normalized = guideline.trim();
		if (normalized.length > 0) {
			addGuideline(normalized);
		}
	}

	// Always include these
	addGuideline("Be concise in your responses");
	addGuideline("Show file paths clearly when working with files");

	const guidelines = guidelinesList.map((g) => `- ${g}`).join("\n");

	return `You are an expert coding assistant operating inside pi, a coding agent harness. You help users by reading files, executing commands, editing code, and writing new files.

Available tools:
${toolsList}

In addition to the tools above, you may have access to other custom tools depending on the project.

Guidelines:
${guidelines}

Pi documentation (read only when the user asks about pi itself, its SDK, extensions, themes, skills, or TUI):
- Main documentation: ${readmePath}
- Additional docs: ${docsPath}
- Examples: ${examplesPath} (extensions, custom tools, SDK)
- When asked about: extensions (docs/extensions.md, examples/extensions/), themes (docs/themes.md), skills (docs/skills.md), prompt templates (docs/prompt-templates.md), TUI components (docs/tui.md), keybindings (docs/keybindings.md), SDK integrations (docs/sdk.md), custom providers (docs/custom-provider.md), adding models (docs/models.md), pi packages (docs/packages.md)
- When working on pi topics, read the docs and examples, and follow .md cross-references before implementing
- Always read pi .md files completely and follow links to related docs (e.g., tui.md for TUI API details)`;
}
