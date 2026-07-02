/**
 * switch_scramjet_model tool — agent-callable harness model switch (issue 244, Stage 4).
 *
 * The agent calls this to change the active harness model through the canonical
 * model-selection path (`pi.setModel`). Unlike a notification-only tool, this
 * performs a real switch: the model resolved from the registry is passed to
 * `pi.setModel`, whose success/failure is surfaced back to the agent as the tool
 * result. The tool's own tool row is the transcript record of the change.
 *
 * Three failure modes, all reported as soft-text errors (never thrown, never a
 * silent fallback), leaving the current model unchanged:
 * - Unknown model: `modelRegistry.find` returns nothing. No `pi.setModel` call is
 *   made (so no `model_select`, no history entry). The error lists the available
 *   catalog so the agent can retry with a valid target.
 * - setModel throws: the target resolves but `pi.setModel` rejects (e.g. persist
 *   failure). The error surfaces the exception message.
 * - Unauthorized model: the target resolves but has no configured auth, so
 *   `pi.setModel` returns false. The error explains the missing auth.
 *
 * Suppression-flag ordering: `pi.setModel` emits and awaits `model_select`
 * before it resolves, so `state.suppressNextModelNotify` must be set *before*
 * the call. The `model_select` handler in `model-change-notice.ts` reads and
 * clears the flag within that await — so an agent-initiated switch does not
 * also emit a redundant user-change notice.
 */

import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { Type } from "typebox";
import type { ScramjetState } from "./types.js";

const PROMPT_SNIPPET =
	"You have access to `switch_scramjet_model` to change the active model mid-session. " +
	"Provide the target model's `provider` and `model` ID; the switch takes effect on the next " +
	"model request and is recorded as a tool row. An unknown or unauthorized target returns an " +
	"error and leaves the current model unchanged (no silent fallback).";

const PARAMETERS = Type.Object({
	provider: Type.String({
		description: "Provider of the target model, e.g. 'anthropic' or 'openai'.",
	}),
	model: Type.String({
		description: "Model ID to switch to, e.g. 'claude-opus-4-8'.",
	}),
});

function formatAvailableCatalog(models: readonly { provider: string; id: string; name: string }[]): string {
	if (models.length === 0) {
		return "No models with configured authentication are available.";
	}
	const lines = models
		.map((m) => `- ${m.provider}/${m.id} (${m.name})`)
		.sort((a, b) => a.localeCompare(b))
		.join("\n");
	return `Available models (provider/id):\n${lines}`;
}

export function registerModelSwitchTool(pi: ExtensionAPI, state: ScramjetState) {
	pi.registerTool({
		name: "switch_scramjet_model",
		label: "Switch Scramjet Model",
		description:
			"Change the active harness model for this session. Provide the target model's provider and ID. " +
			"The switch takes effect on the next model request. An unknown or unauthorized target returns an " +
			"error and leaves the current model unchanged.",
		promptSnippet: PROMPT_SNIPPET,
		parameters: PARAMETERS,
		async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
			const resolved = ctx.modelRegistry.find(params.provider, params.model);
			if (!resolved) {
				const catalog = formatAvailableCatalog(ctx.modelRegistry.getAvailable());
				const text = `Unknown model "${params.provider}/${params.model}". The model was not switched.\n${catalog}`;
				state.logger.warn("model-switch", "switch_scramjet_model called with unknown model", {
					provider: params.provider,
					model: params.model,
				});
				return {
					content: [{ type: "text", text }],
					details: { error: "unknown-model", provider: params.provider, model: params.model },
				};
			}

			// Same-model guard: if the target is already the active model, skip the
			// switch entirely. Without this, setModel's modelsAreEqual early-return
			// skips the model_select emission but the suppression flag would be
			// stranded true, swallowing the next user-initiated model-change notice.
			if (state.currentModel && state.currentModel.id === resolved.id) {
				state.logger.debug("model-switch", "model already active, no-op", {
					provider: resolved.provider,
					model: resolved.id,
				});
				return {
					content: [
						{
							type: "text",
							text: `Already on ${resolved.name} (${resolved.provider}/${resolved.id}). No switch needed.`,
						},
					],
					details: {
						switched: false,
						provider: resolved.provider,
						model: resolved.id,
						name: resolved.name,
						reason: "already-active",
					},
				};
			}

			// Set the suppression flag before the switch: setModel emits and awaits
			// model_select synchronously, so the Stage 5 handler must see the flag
			// already set when it runs inside the call below.
			state.suppressNextModelNotify = true;

			let switched: boolean;
			try {
				switched = await pi.setModel(resolved);
			} catch (error) {
				state.suppressNextModelNotify = false;
				const message = error instanceof Error ? error.message : String(error);
				state.logger.warn("model-switch", "switch_scramjet_model setModel threw", {
					provider: resolved.provider,
					model: resolved.id,
					error: message,
				});
				return {
					content: [
						{
							type: "text",
							text: `Failed to switch to ${resolved.name}: ${message}. The model was not changed.`,
						},
					],
					details: { error: "switch-failed", provider: resolved.provider, model: resolved.id, message },
				};
			}

			if (!switched) {
				state.suppressNextModelNotify = false;
				const text =
					`Cannot switch to ${resolved.name} (${resolved.provider}/${resolved.id}): ` +
					"no API key or authorization is configured for that provider. The model was not changed.";
				state.logger.warn("model-switch", "switch_scramjet_model rejected (no auth)", {
					provider: resolved.provider,
					model: resolved.id,
				});
				return {
					content: [{ type: "text", text }],
					details: { error: "no-auth", provider: resolved.provider, model: resolved.id },
				};
			}

			state.logger.debug("model-switch", "model switched via tool", {
				provider: resolved.provider,
				model: resolved.id,
			});
			return {
				content: [
					{
						type: "text",
						text: `Switched to ${resolved.name} (ID: ${resolved.id}, provider: ${resolved.provider}).`,
					},
				],
				details: { switched: true, provider: resolved.provider, model: resolved.id, name: resolved.name },
			};
		},
	});
}
