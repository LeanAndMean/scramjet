// SCRAMJET-DIVERGENCE: scramjet-command block rendering (issue 82)

export interface ParsedScramjetCommandBlock {
	name: string;
	content: string;
	userMessage?: string;
	userContext?: string;
}

export function parseScramjetCommandBlock(text: string): ParsedScramjetCommandBlock | null {
	const match = text.match(/^<scramjet-command name="([^"]+)">\n([\s\S]*?)\n<\/scramjet-command>(?:\n\n([\s\S]+))?$/);
	if (!match) return null;

	const name = match[1];
	const content = match[2];
	const userMessage = match[3]?.trim() || undefined;

	let userContext: string | undefined;
	const ctxMatch = content.match(/<user-context>\n?([\s\S]*?)\n?<\/user-context>/);
	if (ctxMatch) {
		const extracted = ctxMatch[1].trim();
		if (extracted) userContext = extracted;
	}

	return { name, content, userMessage, userContext };
}
