import type { Candidate, NextStepPolicy } from "./types.ts";

const CLOSE_TAG = "</scramjet-next-step>";
const ESCAPED_CLOSE_TAG = "<\\/scramjet-next-step>";

function safe(s: string): string {
	return s.split(CLOSE_TAG).join(ESCAPED_CLOSE_TAG);
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
				`\`${safe(policy.target)}\` will run unconditionally after this turn. ` +
				`You do not need to set next_step on task_complete.`;
			break;
		case "closed":
			body =
				`The command \`${id}\` declares a \`closed\` next-step policy.\n` +
				`When you call task_complete, pick one of these candidates for next_step.command:\n` +
				`${formatCandidates(policy.candidates)}\n` +
				`If none apply, omit next_step entirely to stop the chain.`;
			break;
		case "open": {
			const blacklistLine = policy.blacklist?.length
				? `\nDo not pick: ${policy.blacklist.map(safe).join(", ")}.`
				: "";
			body =
				`The command \`${id}\` declares an \`open\` next-step policy.\n` +
				`Suggested candidates for next_step.command:\n` +
				`${formatCandidates(policy.candidates)}\n` +
				`You may also pick any other slash command if it fits the work.${blacklistLine}\n` +
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
