// SCRAMJET-DIVERGENCE: bound SDK listeners and transport lifetime without aborting the caller (#587).
export function createProviderAbortScope(caller?: AbortSignal): { signal: AbortSignal; dispose(): void } {
	const controller = new AbortController();
	const forwardAbort = () => controller.abort(caller?.reason);
	if (caller?.aborted) {
		forwardAbort();
	} else {
		caller?.addEventListener("abort", forwardAbort, { once: true });
	}
	return {
		signal: controller.signal,
		dispose() {
			try {
				controller.abort();
			} finally {
				caller?.removeEventListener("abort", forwardAbort);
			}
		},
	};
}
