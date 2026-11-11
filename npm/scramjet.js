#!/usr/bin/env node
// Runs the scramjet program from the package npm installed for this platform.
"use strict";
const { spawn } = require("child_process");

const packages = {
  "linux x64": "@leanandmean/scramjet-linux-x64",
  "linux arm64": "@leanandmean/scramjet-linux-arm64",
  "darwin arm64": "@leanandmean/scramjet-darwin-arm64",
  "darwin x64": "@leanandmean/scramjet-darwin-x64",
  "win32 x64": "@leanandmean/scramjet-win32-x64",
};
const platform = `${process.platform} ${process.arch}`;
const name = packages[platform];
if (!name) {
  console.error(
    `scramjet: no build for ${platform}; see https://github.com/LeanAndMean/scramjet#install`,
  );
  process.exit(1);
}
let program;
try {
  program = require.resolve(`${name}/bin/scramjet${process.platform === "win32" ? ".exe" : ""}`);
} catch {
  console.error(
    `scramjet: ${name} is not installed (npm leaves it out with --omit=optional); ` +
      "install again with `npm install -g @leanandmean/scramjet`",
  );
  process.exit(1);
}

const child = spawn(program, process.argv.slice(2), { stdio: "inherit" });
// The terminal sends Ctrl+C's and Ctrl+\'s signals to the program too, which handles them;
// signals sent to this process alone are passed on. SIGTSTP keeps its default, so this process
// stops with the program's job.
const forwarded = ["SIGTERM", "SIGHUP"];
const ignored = process.platform === "win32" ? ["SIGINT"] : ["SIGINT", "SIGQUIT"];
for (const signal of ignored) process.on(signal, () => {});
for (const signal of forwarded) process.on(signal, () => child.kill(signal));
child.on("error", (error) => {
  console.error(`scramjet: cannot run ${program}: ${error.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) {
    for (const each of [...ignored, ...forwarded]) process.removeAllListeners(each);
    process.kill(process.pid, signal);
  }
  process.exit(code ?? 1);
});
