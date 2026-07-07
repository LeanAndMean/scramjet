/**
 * Regression guard: Scramjet call sites that create new sessions must NOT pass
 * explicit model/thinkingLevel options. They rely on inherit-by-default behavior
 * in AgentSessionRuntime.newSession() (issue 186, Stage 1), which snapshots the
 * live model before teardown and forwards it through the factory.
 *
 * If a future change adds model parameters to these calls, it means the
 * inherit-by-default contract is being bypassed — investigate before accepting.
 */
import { describe, expect, it, vi } from "vitest";
import { registerClearAlias } from "../src/clear-alias.js";
import { dispatchNextStep } from "../src/next-step-dispatch.js";
import type { NextStep } from "../src/types.js";
import { freshState } from "./helpers.js";

describe("model inherit-by-default regression guard", () => {
	describe("clear-alias — ctx.newSession() called with no arguments", () => {
		it("passes no arguments to ctx.newSession()", async () => {
			const registered: Array<{ name: string; spec: { handler: (...args: any[]) => any } }> = [];
			const pi: any = {
				registerCommand(name: string, spec: any) {
					registered.push({ name, spec });
				},
			};
			registerClearAlias(pi);

			const newSession = vi.fn(async () => {});
			const ctx = { newSession };
			await registered[0].spec.handler(undefined, ctx);

			expect(newSession).toHaveBeenCalledTimes(1);
			expect(newSession).toHaveBeenCalledWith();
		});
	});

	describe("next-step-dispatch — ctx.newSession() called with only { withSession }", () => {
		function minimalCtx() {
			const calls: unknown[] = [];
			const ctx: any = {
				newSession: vi.fn(async (options?: any) => {
					calls.push(options);
					await options?.withSession?.({
						dispatchUserInput: vi.fn(async () => {}),
					});
					return { cancelled: false };
				}),
				dispatchUserInput: vi.fn(async () => {}),
				ui: { notify: vi.fn() },
			};
			return { ctx, calls };
		}

		it("fresh-session dispatch passes only { withSession } — no model or thinkingLevel", () => {
			const state = freshState();
			const step: NextStep = { name: "test:cmd", args: "42", freshSession: true };
			const { ctx, calls } = minimalCtx();

			dispatchNextStep(ctx, state, step, { origin: "agent" });

			expect(ctx.newSession).toHaveBeenCalledTimes(1);
			expect(calls).toHaveLength(1);

			const opts = calls[0] as Record<string, unknown>;
			expect(opts).toBeDefined();
			expect(typeof opts.withSession).toBe("function");
			expect(Object.keys(opts)).toEqual(["withSession"]);
		});

		it("same-session dispatch does not call ctx.newSession()", () => {
			const state = freshState();
			const step: NextStep = { name: "test:cmd", args: "42", freshSession: false };
			const { ctx } = minimalCtx();

			dispatchNextStep(ctx, state, step, { origin: "agent" });

			expect(ctx.newSession).not.toHaveBeenCalled();
			expect(ctx.dispatchUserInput).toHaveBeenCalledTimes(1);
		});
	});
});
