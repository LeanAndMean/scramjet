/**
 * PR indicator: an ambient footer hint showing the current branch's active
 * GitHub PR number (e.g. `PR #72`) when exactly one open PR matches the
 * branch. It shows nothing in every other case — no PR, multiple PRs, an
 * unsupported remote, a missing/unauthenticated CLI, or not a git repo.
 *
 * This is an opportunistic UI hint, NOT workflow state: nothing is journaled
 * and nothing is added to ScramjetState. It shows regardless of /scramjet
 * on|off, because the flag gates workflow *decisions* (closed/open agent-pick,
 * ask user-pick) and an ambient hint is not a decision. The footer surface
 * (ctx.ui.setStatus) is distinct from the transient below-editor countdown
 * widget in auto-continue.ts.
 */

import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

const STATUS_KEY = "scramjet-pr";
export const EXEC_TIMEOUT_MS = 3000;

// Dependency-injection boundary: tests pass a fake keyed by command so
// resolvePr can be exercised without spawning real processes. The exec is
// already cwd-bound by makeExec, so resolvePr needs no cwd of its own.
export type ExecFn = (cmd: string, args: string[]) => Promise<{ stdout: string; code: number }>;

// Resolve the active GitHub PR number for the current branch, or null in every
// "show nothing" case. Failure modes collapse to a silent null along two paths.
// A subprocess *failure* (missing CLI, no remote, no PR, an auth error) resolves
// with a non-zero code, caught by the `code !== 0` checks below. A *timeout*
// does NOT take that path: Pi kills the child with SIGTERM, so the command
// resolves `{ code: 0, killed: true }` (code is coerced from null to 0). A
// timed-out command is instead caught downstream by the empty-stdout guards —
// the `branch === ""` check after the rev-parse and the `JSON.parse` catch on
// the gh output. Pi's exec does not reject for subprocess failures; note,
// though, that `pi.exec` itself can still throw *synchronously* via
// `runtime.assertActive()` before spawning if the extension has gone stale —
// that throw is absorbed by the host runner's per-handler try/catch, not here.
export async function resolvePr(exec: ExecFn): Promise<number | null> {
	const remote = await exec("git", ["remote", "get-url", "origin"]);
	if (remote.code !== 0) return null; // not a git repo / no origin / git missing

	// Forge classify. GitHub-only for now; an unknown remote (GitLab, an SSH
	// host alias, a self-hosted forge) shows nothing.
	// forge-swap point: a future `glab` branch slots in here — key on a GitLab
	// host and render `MR !<iid>` instead of `PR #<n>`.
	if (!remote.stdout.includes("github.com")) return null;

	const branchRes = await exec("git", ["rev-parse", "--abbrev-ref", "HEAD"]);
	if (branchRes.code !== 0) return null;
	const branch = branchRes.stdout.trim();
	if (branch === "" || branch === "HEAD") return null; // detached HEAD / no branch

	// `gh pr list` + array-count is required over `gh pr view`: pr view silently
	// picks one canonical PR and cannot signal multiplicity, so it can't honor
	// the "multiple -> nothing" rule. `--state open` makes a branch whose PRs are
	// all merged show nothing, matching the "active PR" wording.
	const prRes = await exec("gh", ["pr", "list", "--head", branch, "--state", "open", "--json", "number"]);
	if (prRes.code !== 0) return null; // gh missing / unauthenticated / errored

	try {
		const parsed = JSON.parse(prRes.stdout);
		// Exactly one match is the only "show" case; zero or multiple show nothing.
		if (Array.isArray(parsed) && parsed.length === 1 && typeof parsed[0]?.number === "number") {
			return parsed[0].number;
		}
	} catch {
		// Malformed JSON — treat as no result.
	}
	return null;
}

export function registerPrIndicator(pi: ExtensionAPI): void {
	// Closure-local cache (NOT module-level): this state is read only within this
	// module, so closure scope is the correct level, and it sidesteps cross-test
	// state leakage — each registerPrIndicator() call gets a fresh cache without a
	// vi.resetModules dance. Only the branch is cached: it gates the agent_end gh
	// lookup. The resolved PR is not cached because nothing reads it back.
	let cachedBranch: string | null = null;

	const makeExec =
		(cwd: string): ExecFn =>
		async (cmd, args) => {
			const res = await pi.exec(cmd, args, { cwd, timeout: EXEC_TIMEOUT_MS });
			return { stdout: res.stdout, code: res.code };
		};

	// Resolve and paint the footer. Records the branch so the next agent_end can
	// skip the gh lookup when the branch is unchanged. resolvePr re-reads the
	// branch internally; the extra local git spawn is negligible and keeps
	// resolvePr self-contained and unit-testable.
	async function refresh(ctx: ExtensionContext): Promise<void> {
		// No footer surface in print/RPC mode (hasUI === false): skip the git/gh
		// spawns and the setStatus no-op entirely. Mirrors auto-continue.ts.
		if (!ctx.hasUI) return;
		const exec = makeExec(ctx.cwd);
		const branchRes = await exec("git", ["rev-parse", "--abbrev-ref", "HEAD"]);
		cachedBranch = branchRes.code === 0 ? branchRes.stdout.trim() : null;
		const pr = await resolvePr(exec);
		ctx.ui.setStatus(STATUS_KEY, pr !== null ? `PR #${pr}` : undefined);
	}

	const onSession = async (_event: unknown, ctx: ExtensionContext) => {
		await refresh(ctx);
	};
	// session_start (fresh load or resume) and session_tree (branch switch within
	// a session) — the same dual-hook restore pattern history.ts uses.
	pi.on("session_start", onSession);
	pi.on("session_tree", onSession);

	// On agent_end, gate the expensive gh call behind a cheap local branch read:
	// only re-resolve when the branch actually changed (e.g. the agent checked out
	// a different branch). Most turns cost just the git spawn. The footer is
	// eventually-consistent for same-branch changes: a PR opened on the current
	// branch mid-turn does not change the branch (`gh pr create` keeps HEAD put),
	// so it surfaces on the next session_start/session_tree rather than here.
	// before_agent_start is deliberately not used — it fires after the user
	// submits, too late to inform the message being composed.
	pi.on("agent_end", async (_event: unknown, ctx: ExtensionContext) => {
		if (!ctx.hasUI) return;
		const exec = makeExec(ctx.cwd);
		const branchRes = await exec("git", ["rev-parse", "--abbrev-ref", "HEAD"]);
		if (branchRes.code !== 0) return;
		if (branchRes.stdout.trim() === cachedBranch) return;
		await refresh(ctx);
	});
}
