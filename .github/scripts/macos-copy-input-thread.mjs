import { parentPort, workerData } from "node:worker_threads";
import { readSync } from "node:fs";
import { StringDecoder } from "node:string_decoder";
import koffi from "koffi";
import { StdinBuffer } from "../../packages/tui/dist/stdin-buffer.js";
import { startDiagnosticHotkey } from "./macos-copy-koffi.mjs";

const control = new Int32Array(workerData.control);
let focused = false;
let epoch = 0;
const parser = new StdinBuffer({ timeout: 10 });
parser.setMouseReporting(true);
const stopNative = startDiagnosticHotkey(workerData.target, {
    eligible: () => focused,
    onCopy: (event) => parentPort.postMessage({ kind: "copy", event: { ...event, focusEpoch: epoch } }),
});
parser.on("data", (data) => {
    if (data === "\x1b[O") { focused = false; epoch++; }
    if (data === "\x1b[I" || /^\x1b\[<0;\d+;\d+M$/.test(data)) { focused = true; epoch++; }
    Atomics.store(control, 2, epoch);
    Atomics.store(control, 3, focused ? 1 : 0);
    stopNative.refresh();
    parentPort.postMessage({ kind: "data", data });
});
parser.on("paste", (data) => parentPort.postMessage({ kind: "paste", data }));
const libc = koffi.load("/usr/lib/libSystem.B.dylib");
const PollFD = koffi.struct({ fd: "int", events: "short", revents: "short" });
const poll = libc.func("poll", "int", [koffi.inout(koffi.pointer(PollFD)), "uint32", "int"]);
const buffer = Buffer.alloc(16384);
const decoder = new StringDecoder("utf8");
const reader = setInterval(() => {
    for (let n = 0; n < 16; n++) {
        const descriptor = { fd: 0, events: 1, revents: 0 };
        if (poll(descriptor, 1, 0) <= 0 || !(descriptor.revents & 1)) break;
        let count;
        try { count = readSync(0, buffer, 0, buffer.length, null); }
        catch (error) { if (error.code === "EAGAIN" || error.code === "EINTR") break; throw error; }
        if (count === 0) break;
        parser.process(decoder.write(buffer.subarray(0, count)));
    }
}, 3);
const acknowledge = (id) => { Atomics.store(control, 0, id); Atomics.notify(control, 0); };
parentPort.on("message", (message) => {
    if (message.kind === "osc") parser.holdOscInput(message.hold);
    if (message.kind === "stop") {
        clearInterval(reader);
        stopNative();
        parser.destroy();
        acknowledge(message.id);
        parentPort.close();
        return;
    }
    acknowledge(message.id);
});
acknowledge(1);
