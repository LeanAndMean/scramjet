/**
 * Renders the <scramjet-next-step> policy block and the post-response status
 * probe (issue 84). `buildNextStepBlock` describes the active command's `next:`
 * policy to the agent — which candidates are valid for the `next_steps[]`
 * payload of `report_scramjet_command_status`. `buildProbeMessage` wraps that block
 * with the hardcoded status-check preamble that asks the agent to report
 * command status. Pure functions: one branch per policy mode. The close-tag
 * escape is case-insensitive (S4) so an attacker-controlled hint cannot
 * smuggle a literal close tag in a different case.
 */
import type { Candidate, NextStepPolicy } from "./types.js";

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
	return candidates.map((c, index) => `  - [${index}] ${safe(c.name)}${formatHint(c.hint)}`).join("\n");
}

function recommendationRule(scramjetEnabled: boolean): string {
	return scramjetEnabled
		? "With `/scramjet on`, set `recommended_next_step` to the zero-based index only when the recommended entry's message is a slash command; do not set it for a non-command message."
		: "With `/scramjet off`, set `recommended_next_step` to the zero-based index of the best option to show the user; Scramjet will not auto-dispatch it.";
}

export function buildNextStepBlock(policy: NextStepPolicy, commandId: string, scramjetEnabled = true): string {
	const id = safe(commandId);
	let body: string;
	switch (policy.mode) {
		case "forced":
			body =
				`The command \`${id}\` declares a \`forced\` next-step: ` +
				`\`${safe(policy.target)}\` will run after this command completes. ` +
				`You may add a single next_steps entry only to pass arguments or fresh_session to that declared target; ` +
				`its message must be \`/${safe(policy.target)}\` followed by any arguments. ` +
				`Omit next_steps if no runtime arguments need to be passed.`;
			break;
		case "closed":
			body =
				`The command \`${id}\` declares a \`closed\` next-step policy.\n` +
				`Each next_steps entry has a \`message\` field containing the next action as a slash command: start with \`/\`, use one of these zero-based candidates, and include any arguments (e.g., \`/<candidate> <args>\`):\n` +
				`${formatCandidates(policy.candidates)}\n` +
				`Entries may reuse the same command with different arguments to offer meaningful variants.\n` +
				`Set \`fresh_session: true\` when the command should run in a clean session.\n` +
				`Each entry must include reason before you set recommended_next_step.\n` +
				`${recommendationRule(scramjetEnabled)}\n` +
				`If none apply, omit next_steps and recommended_next_step entirely to stop the chain.`;
			break;
		case "open": {
			const blacklistLine = policy.blacklist?.length
				? `\nDo not pick slash commands for: ${policy.blacklist.map(safe).join(", ")}.`
				: "";
			const candidatesLine = policy.candidates.length
				? `Suggested commands with zero-based indexes:\n${formatCandidates(policy.candidates)}\n`
				: "No suggested commands are listed.\n";
			body =
				`The command \`${id}\` declares an \`open\` next-step policy.\n` +
				candidatesLine +
				`You may pick any slash command if it fits the work.${blacklistLine}\n` +
				`Each next_steps entry has a \`message\` field containing the next action.\n` +
				`For slash commands, start with \`/\` and include any arguments (e.g., \`/mach12:pr-merge 113\`).\n` +
				`For non-command follow-ups, write the message text directly.\n` +
				`Set \`fresh_session: true\` when the command should run in a clean session.\n` +
				`Each entry must include reason before you set recommended_next_step.\n` +
				`${recommendationRule(scramjetEnabled)}\n` +
				`Omit next_steps and recommended_next_step entirely to stop the chain.`;
			break;
		}
		case "ask": {
			const hintLine = policy.hint ? `\n${safe(policy.hint.trim())}` : "";
			body =
				`The command \`${id}\` declares an \`ask\` next-step policy. ` +
				`The chain will pause for the user after this turn. ` +
				`Do not include next_steps.${hintLine}`;
			break;
		}
	}
	return `<scramjet-next-step>\n${body}\n</scramjet-next-step>`;
}

// Post-response probe message (issue 84, issue 128, issue 156). The agent
// either reports status or requests user input before continuing work. No
// user-controlled text is interpolated into the preamble; buildNextStepBlock
// owns escaping for the policy portion. The probe fires in a separate turn
// after the command's normal user-facing answer.
export function buildProbeMessage(policy: NextStepPolicy, commandId: string, scramjetEnabled = true): string {
	const id = safe(commandId);
	const preamble =
		`Scramjet status check for \`${id}\`.\n\n` +
		"Choose one route \u2014 do not write prose unless you continue command work after user input.\n\n" +
		"`report_scramjet_command_status` \u2014 report your status and stop the probe turn:\n" +
		"- `continuing` \u2014 you have more work to do (not blocked, not waiting for input, not finished)\n" +
		"- `completed` \u2014 the command's work is done and your answer was already delivered\n" +
		"- `blocked` \u2014 cannot proceed (error, missing dependency, authorization)\n" +
		"- `incomplete` \u2014 stopped without clean completion\n\n" +
		"`get_scramjet_user_input` \u2014 if you need user input before continuing:\n" +
		"- successful `confirm`/`select` responses return in this probe turn; continue command work in this turn\n" +
		"- `freetext` and cancelled `confirm`/`select` terminate this turn and park the command at `waiting`; do not try to continue same-turn after those paths";
	return `${preamble}\n\n${buildNextStepBlock(policy, commandId, scramjetEnabled)}`;
}
