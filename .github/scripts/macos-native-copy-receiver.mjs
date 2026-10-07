import { createRequire, findPackageJSON } from "node:module";
import { dirname, join } from "node:path";
import { realpathSync, renameSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { isMainThread, Worker, workerData } from "node:worker_threads";

if (process.platform !== "darwin") throw new Error("Native receiver requires macOS");
if (isMainThread) {
	const [target, installedRoot] = process.argv.slice(2);
	if (!target || !installedRoot) throw new Error("usage: macos-native-copy-receiver.mjs <receipt> <installed-root>");
	const worker = new Worker(new URL(import.meta.url), { workerData: { target, installedRoot } });
	worker.on("error", (error) => { console.error(error); process.exitCode = 1; });
	worker.on("exit", (code) => { if (code) process.exitCode = code; });
} else {
	const { target, installedRoot } = workerData;
	const base = pathToFileURL(join(realpathSync(installedRoot), "package.json"));
	const tui = dirname(findPackageJSON("@leanandmean/tui", base));
	const adapterPath = realpathSync(join(tui, "dist/macos-input-worker.js"));
	const koffi = createRequire(join(tui, "package.json"))("koffi");
	const { createNative, MacosInputReader } = await import(pathToFileURL(adapterPath).href);
	const services = koffi.load("/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices");
	const capabilities = Object.fromEntries(Object.entries({ accessibility: "AXIsProcessTrusted", listenEvents: "CGPreflightListenEventAccess", postEvents: "CGPreflightPostEventAccess", screenCapture: "CGPreflightScreenCaptureAccess" }).map(([key, name]) => [key, services.func(`bool ${name}()`)()]));
	const receipt = { pid: process.pid, architecture: process.arch, adapterPath, capabilities, permissionRequestsMade: false, notices: [] };
	const record = () => { writeFileSync(`${target}.tmp`, JSON.stringify(receipt)); renameSync(`${target}.tmp`, target); };
	if (Object.values(capabilities).some(Boolean)) {
		receipt.error = "Receiver inherited permission; denied-permission evidence unavailable";
		record();
		throw new Error(receipt.error);
	}
	let exclusive = false;
	const native = createNative({ ...koffi, load(path) {
		const library = koffi.load(path);
		return { func(...args) {
			const call = library.func(...args);
			return args[0] === "RegisterEventHotKey" ? (code, modifiers, id, target, options, result) => call(code, modifiers, id, target, exclusive ? 1 : options, result) : call;
		} };
	} }, 0);
	let registered = false;
	let releaseUnknown = false;
	let timer;
	let deadline;
	function stop(error) {
		clearInterval(timer);
		clearTimeout(deadline);
		if (error) receipt.error = String(error);
		if (registered && !releaseUnknown) { receipt.unregister = native.unregister(); registered = receipt.unregister !== 0; releaseUnknown = registered; }
		if (!registered && !error) {
			exclusive = false;
			try {
				const shared = new Int32Array(new SharedArrayBuffer(24));
				const messages = [];
				let input = Buffer.from("\x1b[I");
				const reader = new MacosInputReader(shared, {
					read() { const value = input; input = null; return value; },
					send(message) { messages.push(message); },
					native: { ...native, foreground: () => true, unregister: () => -1, pump(copy) { native.pump(copy); copy(receipt.notices[0]); } },
				});
				reader.command({ kind: "mouse", enabled: true });
				reader.command({ kind: "lease", lease: 1 });
				reader.command({ kind: "commit" });
				reader.tick();
				registered = Atomics.load(shared, 2) === 1;
				receipt.staleCapturedIdentityRejected = registered && !messages.some((message) => message.kind === "copy");
				try { reader.command({ kind: "lease", lease: 0 }); } catch (failure) { receipt.injectedReleaseFailure = failure.message; }
				try { reader.command({ kind: "stop" }); } catch (failure) { receipt.repeatedReleaseFailure = failure.message; }
				receipt.failedReleaseWithheldRegistration = Atomics.load(shared, 2) === 0;
				receipt.failureQualification = "Real Carbon registration; injected pre-mutation unregister failure and replay of captured stale ID; fixture explicitly cleans its known native ownership.";
			} catch (failure) { receipt.error = String(failure); }
			if (registered) { receipt.challengeUnregister = native.unregister(); registered = receipt.challengeUnregister !== 0; }
		}
		if (!registered) receipt.dispose = native.dispose();
		receipt.stopped = !registered && receipt.dispose === 0;
		record();
		if (error || !receipt.stopped) process.exitCode = 1;
	}
	try {
		receipt.registration = native.register(608);
		registered = receipt.registration === 0;
		if (!registered) throw new Error(`Receiver registration failed: ${receipt.registration}`);
		record();
		timer = setInterval(() => {
			try {
				native.pump((id) => receipt.notices.push(id));
				if (receipt.notices.length === 1 && !exclusive) {
					if (receipt.notices[0] !== 608) throw new Error("Unexpected default-adapter identity");
					receipt.defaultUnregister = native.unregister();
					if (receipt.defaultUnregister !== 0) { releaseUnknown = true; throw new Error("Default adapter cleanup failed"); }
					registered = false;
					exclusive = true;
					receipt.exclusiveCompetitorOptions = 1;
					receipt.exclusiveRegistration = native.register(609);
					registered = receipt.exclusiveRegistration === 0;
					if (!registered) throw new Error("Exclusive competitor registration failed");
					record();
				} else if (receipt.notices.length > 1) stop(receipt.notices.length === 2 && receipt.notices[1] === 609 ? undefined : "Unexpected exclusive identity");
			} catch (error) { stop(error); }
		}, 4);
		deadline = setTimeout(() => stop("Native gesture timed out"), 30000);
	} catch (error) { stop(error); }
}
