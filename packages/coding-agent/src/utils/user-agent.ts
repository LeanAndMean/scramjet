// SCRAMJET-DIVERGENCE: Renamed from pi-user-agent.ts; returns scramjet product identity
export function getUserAgent(version: string): string {
	const runtime = process.versions.bun ? `bun/${process.versions.bun}` : `node/${process.version}`;
	return `scramjet/${version} (${process.platform}; ${runtime}; ${process.arch})`;
}
