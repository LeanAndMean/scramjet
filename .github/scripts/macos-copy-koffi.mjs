import { readFileSync, writeFileSync, renameSync } from "node:fs";
import koffi from "koffi";
import { fileURLToPath } from "node:url";

export function startDiagnosticHotkey(target) {
const carbon = koffi.load("/System/Library/Frameworks/Carbon.framework/Carbon");
const services = koffi.load("/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices");
const Spec = koffi.struct("DiagnosticEventSpec", { eventClass: "uint32", eventKind: "uint32" });
const ID = koffi.struct("DiagnosticHotKeyID", { signature: "uint32", id: "uint32" });
const PSN = koffi.struct("DiagnosticPSN", { high: "uint32", low: "uint32" });
const Callback = koffi.proto("int32 DiagnosticHandler(void *, void *, void *)");
const getTarget = carbon.func("void *GetApplicationEventTarget()");
const install = carbon.func("InstallEventHandler", "int32", ["void *", koffi.pointer(Callback), "uint32", koffi.pointer(Spec), "void *", koffi.out(koffi.pointer("void *"))]);
const register = carbon.func("RegisterEventHotKey", "int32", ["uint32", "uint32", ID, "void *", "uint32", koffi.out(koffi.pointer("void *"))]);
const unregister = carbon.func("int32 UnregisterEventHotKey(void *)");
const receive = carbon.func("ReceiveNextEvent", "int32", ["uint32", "void *", "double", "uint8", koffi.out(koffi.pointer("void *"))]);
const send = carbon.func("int32 SendEventToEventTarget(void *, void *)");
const dispatcher = carbon.func("void *GetEventDispatcherTarget()");
const release = carbon.func("void ReleaseEvent(void *)");
const front = carbon.func("GetFrontProcess", "int32", [koffi.out(koffi.pointer(PSN))]);
const processID = carbon.func("GetProcessPID", "int32", [koffi.pointer(PSN), koffi.out(koffi.pointer("int32"))]);
const foregroundPid = () => { const psn = {}; const pid = [0]; front(psn); processID(psn, pid); return pid[0]; };
const state = { backend: "koffi", capabilities: { pid: process.pid, permissionRequestsMade: false }, events: [], registration: -1, armed: false };
for (const [key, name] of Object.entries({ accessibilityTrusted: "AXIsProcessTrusted", listenEventAccess: "CGPreflightListenEventAccess", postEventAccess: "CGPreflightPostEventAccess", screenCaptureAccess: "CGPreflightScreenCaptureAccess" })) state.capabilities[key] = services.func(`bool ${name}()` )();
let requested = "";
let ref;
const record = () => { writeFileSync(target + ".tmp", JSON.stringify(state)); renameSync(target + ".tmp", target); };
const cb = koffi.register(() => { state.events.push({ kind: "hotkey", lease: requested, foregroundPid: foregroundPid(), sequence: state.events.length }); record(); return requested === "arm-pass" ? -9874 : 0; }, koffi.pointer(Callback));
const handler = [null];
state.events.push({ kind: "handler", status: install(getTarget(), cb, 1, { eventClass: 0x6b657962, eventKind: 5 }, null, handler) });
const timer = setInterval(() => {
    let command = "";
    try { command = readFileSync(target + ".control", "utf8"); } catch (error) { if (error.code !== "ENOENT") throw error; }
    if (command !== requested) {
        requested = command;
        if (ref) { unregister(ref); ref = null; }
        if (command.startsWith("arm")) { const result = [null]; state.registration = register(8, 256, { signature: 0x5343524d, id: 608 }, getTarget(), 0, result); ref = result[0]; }
        state.armed = Boolean(ref);
        record();
    }
    for (let n = 0; n < 32; n++) { const event = [null]; if (receive(0, null, 0, 1, event) !== 0) break; send(event[0], dispatcher()); release(event[0]); }
}, 10);
const stop = () => { clearInterval(timer); if (ref) unregister(ref); process.exit(0); };
process.on("SIGTERM", stop);
setTimeout(stop, 180_000);
record();
}
if (process.argv[1] === fileURLToPath(import.meta.url)) startDiagnosticHotkey(process.argv[2]);
