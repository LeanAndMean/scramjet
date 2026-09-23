# Catalog review for issue 569

## Stage 1 — generator safety (2026-09-23)

Baseline: `b2bb160d` (post-#568); `src/models.generated.ts` is unchanged. Upstream Pi `earendil-works/pi` HEAD `7fd564cbb78f35f3de14d5382fea692b87ec4026` was inspected on 2026-09-23: `packages/ai/scripts/{generate-models,model-data,check-model-data}.ts`, provider definitions `src/providers/{openrouter,vercel-ai-gateway}.models.ts`, and `test/{model-data-validation,generate-models-strict}.test.ts`. These are comparative observations, not endpoint evidence.

Adopted: reject malformed required/available source sections; validate included source price/limits and normalized identity, cost, limits and collisions before writing; use literal-safe serialization for externally supplied strings; cover rejection and no-write behavior with mocked acquisition. Pi's model-data validator detects duplicate IDs across API groups and checks exact model identity, and its strict-generation test checks failed generation does not alter the canonical artifact. This generator retains one sorted generated file and enforces strict failure by default.

Not adopted: Pi's per-provider TypeScript/JSON shards, manifest, offline data cache, non-strict empty-provider fallback, image/classifier features, or `4096` fallback for missing limits. Those mechanisms either solve different needs or would introduce a second authority or unevidenced limits. The existing runtime `Model` contract and its request-limit validator remain authoritative.

Source-shape policy: `anthropic`, `google`, `openai`, `groq`, and `cerebras` models.dev sections must exist with object-valued `models`; other consumed sections may be absent, but if present must be well-formed. Empty required sections remain possible (for example, the existing OpenAI fallback fixture); a broad but otherwise valid removal is a Stage 2 inventory blocker, not an arbitrary count-threshold error. Included models.dev records require positive context/output and explicit input/output/cache prices; intentionally excluded records (for example deprecated Copilot/Together entries) do not need those fields. All three recognized Together section names are shape-checked. Tool-capable OpenRouter and Vercel records require explicit finite nonnegative prices including cache fields; explicit zero is accepted, missing is unknown and rejected rather than represented as free. Top-level output may be taken from declared endpoint maxima when aggregate output is absent, but no constant ceiling is invented. Providers that omit optional cache prices or output entirely will need a route-specific, evidenced disposition before the live candidate can pass; do not quietly restore the old defaults.

Stage 2 remains to acquire and reconcile a complete live candidate in isolation, recording provider/route evidence, exclusions and unresolved fields. No live feeds or paid endpoint boundary calls were made for Stage 1; passing mocked tests establish only the offline failure boundary, not live-source completeness or endpoint acceptance.

## Stage 2 — acquisition blocked; inventory is not a disposition (2026-09-23)

Authorized live acquisition was attempted in a disposable checkout of `49c1f1209f8129a6dffc84c534da5b1c364ceb14` using `cd packages/ai && npm run generate-models`; it exited before writing a candidate: `models.dev/google/gemini-flash-latest cache write: missing or invalid price`. The checked-in snapshot remains unchanged (SHA-256 `089e634c71fc3b6bbdbb6563114851aa500d633a0d71e9876aa0728a42dd33dc`). This is an **unresolved** price, not evidence that the price is zero or that the model should be removed. No paid endpoint calls were made.

Separate top-level GETs succeeded on 2026-09-23 UTC for `https://models.dev/api.json` (SHA-256 `1737e0753061fd719ca157d575ca25e04ef7d41d381a41c100711351931ac433`), `https://openrouter.ai/api/v1/models` (SHA-256 `196d7dee5d4fbe2936c21fa7a4d2f5b342cb47be899e8f45c1246e7a497e9c9c`), and `https://ai-gateway.vercel.sh/v1/models` (SHA-256 `1c391055363636337b16821926176d36b20345eda47c1eb2e4c5bb79444f7bd8`). Raw feeds were retained only in the disposable checkout; their hashes identify this observation, not a replayable committed input set. The normal generator never reached OpenRouter or Vercel endpoint discovery. Counts below describe top-level source records **before generator exclusions, renaming, overrides or derived routes**; `missing cost` counts tool-flagged records missing at least one of the four input/output/cache price fields, and include records that may later be excluded. These counts cannot be compared directly to normalized snapshot counts or treated as removals/additions.

| Source section | Raw | Tool-flagged | Missing cost |
| --- | ---: | ---: | ---: |
| models.dev/anthropic | 15 | 15 | 0 |
| models.dev/google | 39 | 22 | 22 |
| models.dev/openai | 50 | 41 | 34 |
| models.dev/groq | 16 | 7 | 7 |
| models.dev/cerebras | 2 | 2 | 2 |
| models.dev/amazon-bedrock | 177 | 172 | 63 |
| models.dev/cloudflare-workers-ai | 27 | 18 | 18 |
| models.dev/cloudflare-ai-gateway | 47 | 47 | 32 |
| models.dev/xai | 12 | 7 | 7 |
| models.dev/zai-coding-plan | 7 | 7 | 0 |
| models.dev/mistral | 35 | 32 | 32 |
| models.dev/huggingface | 78 | 76 | 76 |
| models.dev/fireworks-ai | 33 | 33 | 33 |
| models.dev/github-copilot | 32 | 32 | 17 |
| models.dev/minimax | 7 | 7 | 2 |
| models.dev/minimax-cn | 7 | 7 | 2 |
| models.dev/kimi-for-coding | 0 | 0 | 0 |
| models.dev/xiaomi | 9 | 9 | 9 |
| models.dev/opencode | 110 | 110 | 83 |
| models.dev/opencode-go | 40 | 40 | 30 |
| models.dev/together | 0 | 0 | 0 |
| models.dev/togetherai | 39 | 33 | 33 |
| models.dev/together-ai | 0 | 0 | 0 |
| models.dev/moonshotai | 4 | 4 | 4 |
| models.dev/moonshotai-cn | 4 | 4 | 4 |
| OpenRouter aggregate | 456 | 387 | 302 |
| Vercel AI Gateway aggregate | 388 | 245 | 196 |

The post-#568 checked-in snapshot contains 32 provider keys: `amazon-bedrock` 106, `anthropic` 14, `azure-openai-responses` 46, `cerebras` 3, `cloudflare-ai-gateway` 38, `cloudflare-workers-ai` 13, `deepseek` 2, `fireworks` 16, `github-copilot` 41, `google` 16, `google-vertex` 13, `groq` 7, `huggingface` 49, `kimi-coding` 3, `minimax` 2, `minimax-cn` 2, `mistral` 30, `moonshotai` 9, `moonshotai-cn` 9, `openai` 47, `openai-codex` 14, `opencode` 51, `opencode-go` 13, `openrouter` 268, `together` 20, `vercel-ai-gateway` 192, `xai` 8, `xiaomi` 6, `xiaomi-token-plan-ams` 6, `xiaomi-token-plan-cn` 6, `xiaomi-token-plan-sgp` 6, `zai` 6. This is an inventory of existing values, **not** fresh provider verification.

**Blocker / required decision before resuming S2:** strict four-field price validation on every included record (`generate-models.ts`, `price()` and source readers) conflicts with the observed widespread omission of optional cache prices. Do not blanket-replace missing fields with zero or delete whole providers to obtain a successful run. Determine route-specific authoritative prices or documented unsupported cache behavior, and decide whether the current scalar cost contract and generator policy can represent the unresolved cases without falsely asserting free pricing. This may require a bounded plan amendment and user decision before changing the runtime schema or accepted catalog; no such amendment is approved here. Then rerun isolated generation and endpoint acquisition, compare the full normalized candidate with the baseline, and record individual dispositions/evidence/impact as S2 requires. No candidate, exact added/removed/changed inventory, endpoint constraint evidence or complete disposition exists yet; S3 is blocked.
