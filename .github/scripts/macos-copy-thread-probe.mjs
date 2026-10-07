import { isMainThread, Worker, workerData } from "node:worker_threads";
import { startDiagnosticHotkey } from "./macos-copy-koffi.mjs";

if (isMainThread) {
    const worker = new Worker(new URL(import.meta.url), { workerData: { target: process.argv[2] } });
    worker.on("error", (error) => { console.error(error); process.exitCode = 1; });
} else {
    startDiagnosticHotkey(workerData.target);
}
