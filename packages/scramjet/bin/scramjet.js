#!/usr/bin/env node
import "./env-setup.js";

const args = process.argv.slice(2);
const { dispatchEarlyCliCommand } = await import("@leanandmean/coding-agent/early-dispatch");
if (!(await dispatchEarlyCliCommand(args))) {
	const [{ main }, { initScramjet }] = await Promise.all([
		import("@leanandmean/coding-agent"),
		import("../dist/index.js"),
	]);
	await main(args, { builtinInit: initScramjet });
}
