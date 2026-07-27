import xterm from "@xterm/headless";
import type { Terminal as TerminalContract } from "../../src/terminal.js";

export class HeadlessTerminal implements TerminalContract {
	readonly writes: string[] = [];
	private readonly emulator: InstanceType<typeof xterm.Terminal>;
	private pending = Promise.resolve();
	private inputHandler?: (data: string) => void;
	private resizeHandler?: () => void;
	private _columns: number;
	private _rows: number;

	constructor(columns = 40, rows = 8) {
		this._columns = columns;
		this._rows = rows;
		this.emulator = new xterm.Terminal({ cols: columns, rows, scrollback: 1000, allowProposedApi: true });
	}

	start(onInput: (data: string) => void, onResize: () => void): void {
		this.inputHandler = onInput;
		this.resizeHandler = onResize;
	}
	stop(): void {}
	async drainInput(): Promise<void> {}

	write(data: string): void {
		this.writes.push(data);
		this.pending = this.pending.then(() => new Promise<void>((resolve) => this.emulator.write(data, resolve)));
	}

	get columns(): number {
		return this._columns;
	}
	get rows(): number {
		return this._rows;
	}
	get kittyProtocolActive(): boolean {
		return false;
	}

	moveBy(lines: number): void {
		if (lines !== 0) this.write(`\x1b[${Math.abs(lines)}${lines < 0 ? "A" : "B"}`);
	}
	hideCursor(): void {
		this.write("\x1b[?25l");
	}
	showCursor(): void {
		this.write("\x1b[?25h");
	}
	clearLine(): void {
		this.write("\x1b[2K");
	}
	clearFromCursor(): void {
		this.write("\x1b[0J");
	}
	clearScreen(): void {
		this.write("\x1b[2J\x1b[H");
	}
	setTitle(title: string): void {
		this.write(`\x1b]0;${title}\x07`);
	}
	setProgress(): void {}
	holdOscInput(): void {}

	async flush(): Promise<void> {
		await this.pending;
	}

	markWrites(): number {
		return this.writes.length;
	}

	writesSince(mark: number): string {
		return this.writes.slice(mark).join("");
	}

	visibleLines(): string[] {
		const buffer = this.emulator.buffer.active;
		return Array.from(
			{ length: this._rows },
			(_, row) => buffer.getLine(buffer.viewportY + row)?.translateToString(true) ?? "",
		);
	}

	bufferLines(): string[] {
		const buffer = this.emulator.buffer.active;
		return Array.from({ length: buffer.length }, (_, row) => buffer.getLine(row)?.translateToString(true) ?? "");
	}

	cursorPosition(): { row: number; col: number } {
		const buffer = this.emulator.buffer.active;
		return { row: buffer.cursorY, col: buffer.cursorX };
	}

	sendInput(data: string): void {
		this.inputHandler?.(data);
	}

	resize(columns: number, rows: number): void {
		this._columns = columns;
		this._rows = rows;
		this.emulator.resize(columns, rows);
		this.resizeHandler?.();
	}
}
