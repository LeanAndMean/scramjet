import type { SystemPromptSection } from "../types.js";

/**
 * Flattens a system prompt to a single string.
 *
 * Sections are joined with no inserted separator: each section's `text`
 * carries its own leading separator, so flattening an array of sections is
 * byte-identical to the equivalent single-string prompt. String prompts pass
 * through unchanged.
 */
export function flattenSystemPrompt(prompt: string | SystemPromptSection[]): string;
export function flattenSystemPrompt(prompt: string | SystemPromptSection[] | undefined): string | undefined;
export function flattenSystemPrompt(prompt: string | SystemPromptSection[] | undefined): string | undefined {
	if (prompt === undefined) return undefined;
	if (typeof prompt === "string") return prompt;
	return prompt.map((section) => section.text).join("");
}
