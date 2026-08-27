import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "fs";
import { tmpdir } from "os";
import { join } from "path";
import { describe, expect, it } from "vitest";
import { loadSkillsFromDir } from "../src/core/skills.js";
import { buildSystemPromptSections } from "../src/core/system-prompt.js";

describe("skills", () => {
	it("retains over-limit descriptions in loaded state and the model-visible prompt", () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-skill-"));
		const description = `${"a".repeat(1024)}TAIL_MARKER_500`;

		try {
			const skillDir = join(root, "long-description");
			mkdirSync(skillDir);
			writeFileSync(
				join(skillDir, "SKILL.md"),
				`---\nname: long-description\ndescription: ${description}\n---\n\n# Long Description\n`,
			);

			const result = loadSkillsFromDir({ dir: root, source: "path" });
			expect(result.skills).toHaveLength(1);
			expect(result.skills[0].description).toBe(description);
			expect(result.diagnostics).toEqual([
				expect.objectContaining({
					type: "warning",
					message:
						"description exceeds Agent Skills specification limit of 1024 characters (1039); advisory only, full description is retained",
				}),
			]);

			const sections = buildSystemPromptSections({
				cwd: root,
				selectedTools: ["read"],
				skills: result.skills,
			});
			const skillsSection = sections.find((section) => section.id === "skills");
			expect(skillsSection?.text).toContain(`<description>${description}</description>`);
			expect(skillsSection?.text).toContain("TAIL_MARKER_500");
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("counts description length by Unicode code points", () => {
		const root = mkdtempSync(join(tmpdir(), "scramjet-skill-"));

		try {
			for (const [name, description] of [
				["at-limit", "😀".repeat(1024)],
				["over-limit", "😀".repeat(1025)],
			] as const) {
				const skillDir = join(root, name);
				mkdirSync(skillDir);
				writeFileSync(join(skillDir, "SKILL.md"), `---\nname: ${name}\ndescription: ${description}\n---\n`);
			}

			const result = loadSkillsFromDir({ dir: root, source: "path" });
			expect(result.skills).toHaveLength(2);
			expect(result.diagnostics).toEqual([
				expect.objectContaining({
					type: "warning",
					message:
						"description exceeds Agent Skills specification limit of 1024 characters (1025); advisory only, full description is retained",
				}),
			]);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
});
