import { isForgeReadPayload } from "./native-reply.js";
import { controlSafeText } from "./text.js";
import type { ForgeReadDetails, ForgeReadSegmentId } from "./types.js";

function byteSlice(value: string, start: number, end: number): string | null {
	const buffer = Buffer.from(value, "utf8");
	if (start < 0 || end < start || end > buffer.length) return null;
	const slice = buffer.subarray(start, end);
	const decoded = slice.toString("utf8");
	return Buffer.from(decoded, "utf8").equals(slice) ? decoded : null;
}

function record(value: unknown): Record<string, unknown> | null {
	return typeof value === "object" && value !== null && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: null;
}

function text(value: unknown): string | null {
	return typeof value === "string" ? value : null;
}

function githubItems(id: ForgeReadSegmentId, output: string): unknown[] | null {
	let parsed: unknown;
	try {
		parsed = JSON.parse(output);
	} catch {
		return null;
	}
	if (id === "artifact" || id === "parent") return [parsed];
	if (!Array.isArray(parsed)) return null;
	const items: unknown[] = [];
	for (const page of parsed) {
		if (Array.isArray(page)) {
			items.push(...page);
			continue;
		}
		const envelope = record(page);
		const nested = id === "check_runs" ? envelope?.check_runs : id === "status" ? envelope?.statuses : undefined;
		if (!Array.isArray(nested)) return null;
		items.push(...nested);
	}
	return items;
}

function gitlabItems(id: ForgeReadSegmentId, output: string): unknown[] | null {
	if (id === "artifact") {
		try {
			return [JSON.parse(output)];
		} catch {
			return null;
		}
	}
	if (output === "") return [];
	try {
		return output.split("\n").map((line) => JSON.parse(line));
	} catch {
		return null;
	}
}

function actorName(value: unknown): string {
	const actor = record(value);
	return text(actor?.login) ?? text(actor?.username) ?? "unknown";
}

function artifactView(value: unknown, forge: "github" | "gitlab"): string | null {
	const artifact = record(value);
	if (artifact === null) return null;
	const title = text(artifact.title);
	const body = forge === "github" ? text(artifact.body) : text(artifact.description);
	const url = forge === "github" ? text(artifact.html_url) : text(artifact.web_url);
	if (title === null || url === null || (body !== null && typeof body !== "string")) return null;
	const facts = [
		artifact.state === undefined ? null : `state: ${String(artifact.state)}`,
		artifact.draft === undefined ? null : `draft: ${String(artifact.draft)}`,
		url,
	].filter((item): item is string => item !== null);
	return `# ${title}\n\n${facts.join(" · ")}${body === null || body === "" ? "" : `\n\n${body}`}`;
}

function commentsView(values: unknown[], forge: "github" | "gitlab"): string | null {
	const blocks: string[] = [];
	for (const value of values) {
		const comment = record(value);
		if (comment === null) return null;
		const body = forge === "github" ? text(comment.body) : text(comment.body);
		if (body === null) return null;
		blocks.push(`## Comment by ${actorName(comment.user ?? comment.author)}\n\n${body}`);
	}
	return blocks.length === 0 ? "No comments." : blocks.join("\n\n");
}

function diffCounts(diff: string): { additions: number; deletions: number } {
	let additions = 0;
	let deletions = 0;
	let inHunk = false;
	for (const line of diff.split("\n")) {
		if (line.startsWith("@@")) {
			inHunk = true;
			continue;
		}
		if (!inHunk) continue;
		if (line.startsWith("+")) additions++;
		if (line.startsWith("-")) deletions++;
	}
	return { additions, deletions };
}

function tableView(id: ForgeReadSegmentId, values: unknown[], forge: "github" | "gitlab"): string | null {
	const rows: string[][] = [];
	let headers: string[];
	switch (id) {
		case "files":
			headers = ["File", "Status", "+", "-"];
			for (const value of values) {
				const item = record(value);
				if (item === null) return null;
				if (forge === "github") {
					if (typeof item.filename !== "string" || typeof item.status !== "string") return null;
					rows.push([item.filename, item.status, String(item.additions ?? ""), String(item.deletions ?? "")]);
					continue;
				}
				if (
					typeof item.new_path !== "string" ||
					typeof item.diff !== "string" ||
					typeof item.new_file !== "boolean" ||
					typeof item.renamed_file !== "boolean" ||
					typeof item.deleted_file !== "boolean"
				) {
					return null;
				}
				const status = item.new_file
					? "added"
					: item.renamed_file
						? "renamed"
						: item.deleted_file
							? "deleted"
							: "modified";
				const counts = diffCounts(item.diff);
				rows.push([item.new_path, status, String(counts.additions), String(counts.deletions)]);
			}
			break;
		case "commits":
			headers = ["Commit", "Title", "Author"];
			for (const value of values) {
				const item = record(value);
				if (item === null) return null;
				if (forge === "github") {
					const commit = record(item.commit);
					const author = record(commit?.author);
					if (typeof item.sha !== "string" || typeof commit?.message !== "string") return null;
					rows.push([item.sha.slice(0, 12), commit.message.split("\n")[0], text(author?.name) ?? ""]);
					continue;
				}
				if (typeof item.id !== "string" || typeof item.title !== "string") return null;
				rows.push([item.id.slice(0, 12), item.title, text(item.author_name) ?? ""]);
			}
			break;
		case "check_runs":
		case "status":
		case "pipelines":
			headers = ["Check", "Status", "Conclusion"];
			for (const value of values) {
				const item = record(value);
				if (item === null) return null;
				rows.push([
					String(item.name ?? item.context ?? `pipeline ${item.id ?? ""}`),
					String(item.status ?? item.state ?? ""),
					String(item.conclusion ?? ""),
				]);
			}
			break;
		default:
			return JSON.stringify(values, null, 2);
	}
	const divider = headers.map(() => "---");
	return [headers, divider, ...rows].map((row) => `| ${row.join(" | ")} |`).join("\n");
}

export function prettyForgeReply(content: string, details: unknown): string | null {
	if (!isForgeReadPayload(content, details)) return null;
	const receipt = details as ForgeReadDetails;
	const blocks: string[] = [];
	for (const segment of receipt.segments) {
		if (segment.status !== "ok" || segment.coverage?.unit === "bytes") return null;
		const output = byteSlice(content, segment.payload.output.start, segment.payload.output.end);
		if (output === null) return null;
		if (segment.payload.command !== undefined) {
			const command = byteSlice(content, segment.payload.command.start, segment.payload.command.end);
			if (command === null || !command.startsWith("$ ")) return null;
		}
		const values =
			receipt.repository.forge === "github" ? githubItems(segment.id, output) : gitlabItems(segment.id, output);
		if (values === null) return null;
		let block: string | null;
		if (segment.id === "artifact")
			block = values.length === 1 ? artifactView(values[0], receipt.repository.forge) : null;
		else if (segment.id === "comments") block = commentsView(values, receipt.repository.forge);
		else block = tableView(segment.id, values, receipt.repository.forge);
		if (block === null) return null;
		blocks.push(block);
	}
	return controlSafeText(blocks.join("\n\n"));
}

export function rawForgeReply(content: string): string {
	return controlSafeText(content);
}
