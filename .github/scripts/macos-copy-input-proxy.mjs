import { StdinBuffer } from "../../packages/tui/dist/stdin-buffer.js";
import { startDiagnosticHotkey } from "./macos-copy-koffi.mjs";

let focused = false;
const parser = new StdinBuffer({ timeout: 10 });
parser.setMouseReporting(true);
const stopNative = startDiagnosticHotkey(process.argv[2], {
    eligible: () => focused,
    onCopy: (event) => process.send({ kind: "copy", event }),
});
parser.on("data", (data) => {
    if (data === "\x1b[O") focused = false;
    if (data === "\x1b[I" || /^\x1b\[<0;\d+;\d+M$/.test(data)) focused = true;
    stopNative.refresh();
    process.send({ kind: "data", data });
});
parser.on("paste", (data) => process.send({ kind: "paste", data }));
process.stdin.setEncoding("utf8");
process.stdin.on("data", (data) => parser.process(data));
process.stdin.resume();
const stop = () => { stopNative(); parser.destroy(); process.stdin.pause(); process.exit(0); };
process.once("disconnect", stop);
process.once("SIGTERM", stop);
process.on("message", (message) => {
    if (message.kind === "osc") parser.holdOscInput(message.hold);
    if (message.kind === "stop") stop();
});
process.send({ kind: "ready" });
