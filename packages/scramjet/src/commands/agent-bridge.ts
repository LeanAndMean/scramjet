import { lstatSync, mkdirSync, readdirSync, readlinkSync, statSync, symlinkSync, unlinkSync } from "node:fs";
import { platform } from "node:os";
import { dirname, join, resolve } from "node:path";
import { getAgentDir } from "@scramjet/coding-agent";
import type { AgentRegistry } from "../types.js";

export interface BridgeResult {
	targetDir: string | null;
	created: string[];
	pruned: string[];
	warnings: string[];
	skipped: boolean;
}

function isUnder(targetPath: string, roots: readonly string[]): boolean {
	const target = resolve(targetPath);
	for (const root of roots) {
		const r = resolve(root);
		if (target === r) return true;
		if (target.startsWith(`${r}/`)) return true;
	}
	return false;
}

function resolveSymlinkTarget(linkPath: string): string | null {
	try {
		const raw = readlinkSync(linkPath);
		return resolve(dirname(linkPath), raw);
	} catch {
		return null;
	}
}

// Pi's subagent example extension scans `<getAgentDir()>/agents` directly
// at every dispatch — there is no `agents_discover` hook or `registerAgent`
// API to use, and the upstream README documents symlinking as the install
// method. `ownershipRoots` is what makes the bridge non-destructive: a
// symlink in the target dir is treated as scramjet-owned only when its
// resolved target falls under one of these roots, so user-authored files
// at the same name are preserved. Skipped on native Windows to mirror
// `scripts/postinstall.js`: symlinks there require admin or developer
// mode, and a copy fallback would drift silently when seeded files change.
export function ensureAgentBridge(registry: AgentRegistry, ownershipRoots: readonly string[]): BridgeResult {
	const result: BridgeResult = { targetDir: null, created: [], pruned: [], warnings: [], skipped: false };

	if (platform() === "win32") {
		result.skipped = true;
		result.warnings.push(
			"agent bridge skipped on native Windows; install inside WSL to make mach12 subagents discoverable",
		);
		return result;
	}

	let targetDir: string;
	try {
		targetDir = join(getAgentDir(), "agents");
	} catch (err) {
		result.warnings.push(`agent bridge skipped: getAgentDir() failed (${(err as Error).message})`);
		return result;
	}
	result.targetDir = targetDir;

	try {
		mkdirSync(targetDir, { recursive: true });
	} catch (err) {
		result.warnings.push(`agent bridge skipped: cannot create ${targetDir} (${(err as Error).message})`);
		return result;
	}

	for (const def of registry.values()) {
		const linkPath = join(targetDir, `${def.name}.md`);
		const desired = resolve(def.filePath);

		// F9: ENOENT means "link does not exist" → fall into the create path.
		// Other errors (EACCES, EIO, …) cannot safely be classified as absent;
		// surface them as a warning and skip this entry so we don't end up
		// overwriting whatever is actually there.
		let stat: ReturnType<typeof lstatSync> | null = null;
		try {
			stat = lstatSync(linkPath);
		} catch (err) {
			const code = (err as NodeJS.ErrnoException).code;
			if (code !== "ENOENT") {
				result.warnings.push(
					`agent bridge: could not lstat ${linkPath} (${code ?? "unknown"}: ${(err as Error).message}); skipping`,
				);
				continue;
			}
			stat = null;
		}

		if (stat === null) {
			try {
				symlinkSync(desired, linkPath);
				result.created.push(def.name);
			} catch (err) {
				// EEXIST means another scramjet process created the link
				// between our lstat and symlink — the state is now correct,
				// so don't surface that as a warning. Other errors are real.
				if ((err as NodeJS.ErrnoException).code !== "EEXIST") {
					result.warnings.push(
						`agent bridge: could not link ${linkPath} -> ${desired} (${(err as Error).message})`,
					);
				}
			}
			continue;
		}

		if (stat.isSymbolicLink()) {
			const existingTarget = resolveSymlinkTarget(linkPath);
			if (existingTarget === desired) continue;
			if (existingTarget !== null && isUnder(existingTarget, ownershipRoots)) {
				try {
					unlinkSync(linkPath);
					symlinkSync(desired, linkPath);
					result.created.push(def.name);
				} catch (err) {
					result.warnings.push(`agent bridge: could not refresh ${linkPath} (${(err as Error).message})`);
				}
				continue;
			}
			result.warnings.push(
				`agent bridge: ${linkPath} is a symlink to ${existingTarget ?? "<unreadable>"} (outside scramjet's data dirs); leaving the user's symlink in place`,
			);
			continue;
		}

		result.warnings.push(`agent bridge: ${linkPath} exists as a non-symlink; leaving the user's file in place`);
	}

	let entries: string[] = [];
	try {
		entries = readdirSync(targetDir);
	} catch (err) {
		result.warnings.push(`agent bridge: could not scan ${targetDir} for prune pass (${(err as Error).message})`);
		return result;
	}

	for (const entry of entries) {
		if (!entry.endsWith(".md")) continue;
		const linkPath = join(targetDir, entry);
		// F9: ENOENT is fine (entry vanished mid-scan); non-ENOENT means we
		// can't classify and should not silently skip.
		let stat: ReturnType<typeof lstatSync>;
		try {
			stat = lstatSync(linkPath);
		} catch (err) {
			const code = (err as NodeJS.ErrnoException).code;
			if (code !== "ENOENT") {
				result.warnings.push(
					`agent bridge: could not lstat ${linkPath} during prune (${code ?? "unknown"}: ${(err as Error).message})`,
				);
			}
			continue;
		}
		if (!stat.isSymbolicLink()) continue;
		const target = resolveSymlinkTarget(linkPath);
		if (target === null) continue;
		if (!isUnder(target, ownershipRoots)) continue;
		try {
			statSync(target);
			continue;
		} catch (err) {
			// Only prune when the target is genuinely gone (ENOENT). EACCES,
			// ENOTDIR, etc. mean the target exists but is temporarily
			// inaccessible — pruning would destroy a live link.
			if ((err as NodeJS.ErrnoException).code !== "ENOENT") {
				result.warnings.push(`agent bridge: could not stat ${target} (${(err as Error).message}); skipping prune`);
				continue;
			}
		}
		try {
			unlinkSync(linkPath);
			result.pruned.push(entry.slice(0, -".md".length));
		} catch (err) {
			result.warnings.push(`agent bridge: failed to prune dangling ${linkPath} (${(err as Error).message})`);
		}
	}

	return result;
}
