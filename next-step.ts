/**
 * Renders the <scramjet-next-step> instruction block injected into the
 * agent's first user message by task-complete.ts when the active command
 * declares a `next:` policy. Pure function: one branch per policy mode.
 * The close-tag escape is case-insensitive (S4) so an attacker-controlled
 * hint cannot smuggle a literal close tag in a different case.
 */
import type { Candidate, NextStepPolicy } from "./types.ts";

const ESCAPED_CLOSE_TAG = "<\\/scramjet-next-step>";
// Match any case of </scramjet-next-step>. S4: an earlier version did a
// literal lowercase split, which left </SCRAMJET-NEXT-STEP> (or any mixed
// case) unescaped — trivial prompt-injection bypass.
const CLOSE_TAG_RE = /<\/scramjet-next-step>/gi;

function safe(s: string): string {
	return s.replace(CLOSE_TAG_RE, ESCAPED_CLOSE_TAG);
}

function formatHint(hint: string | undefined): string {
	return hint ? ` — ${safe(hint.trim().replace(/\s+/g, " "))}` : "";
}

function formatCandidates(candidates: Candidate[]): string {
	return candidates.map((c) => `  - ${safe(c.name)}${formatHint(c.hint)}`).join("\n");
}

export function buildNextStepBlock(policy: NextStepPolicy, commandId: string): string {
	const id = safe(commandId);
	let body: string;
	switch (policy.mode) {
		case "forced":
			body =
				`The command \`${id}\` declares a \`forced\` next-step: ` +
				`\`${safe(policy.target)}\` will run after you call task_complete. ` +
				`Do not set next_step on task_complete; the harness already knows the target.`;
			break;
		case "closed":
			body =
				`The command \`${id}\` declares a \`closed\` next-step policy.\n` +
				`When you call task_complete, pick one of these candidates for next_step.name (bare, no leading slash):\n` +
				`${formatCandidates(policy.candidates)}\n` +
				`If none apply, omit next_step entirely to stop the chain.`;
			break;
		case "open": {
			const blacklistLine = policy.blacklist?.length
				? `\nDo not pick: ${policy.blacklist.map(safe).join(", ")}.`
				: "";
			const candidatesLine = policy.candidates.length
				? `Suggested candidates for next_step.name (bare, no leading slash):\n${formatCandidates(policy.candidates)}\n`
				: "No suggested candidates are listed for next_step.name.\n";
			body =
				`The command \`${id}\` declares an \`open\` next-step policy.\n` +
				candidatesLine +
				`You may pick any slash command if it fits the work.${blacklistLine}\n` +
				`Omit next_step entirely to stop the chain.`;
			break;
		}
		case "ask": {
			const hintLine = policy.hint ? `\n${safe(policy.hint.trim())}` : "";
			body =
				`The command \`${id}\` declares an \`ask\` next-step policy. ` +
				`The chain will pause for the user after this turn. ` +
				`Do not set next_step on task_complete.${hintLine}`;
			break;
		}
	}
	return `<scramjet-next-step>\n${body}\n</scramjet-next-step>`;
}
