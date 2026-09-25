> Scramjet can create TUI components. Ask it to build one for your use case.

# TUI Components

Extensions and custom tools can render custom TUI components for interactive user interfaces. This page covers the component system and available building blocks.

**Source:** [`@leanandmean/tui`](https://github.com/earendil-works/pi-mono/tree/main/packages/tui)

## Transcript Rendering

**Scramjet divergence source:** [`packages/tui/src/tui.ts`](../../tui/src/tui.ts)

Interactive mode uses an application-managed retained viewport in the alternate screen. The rightmost scrollbar owns browsing of the current transcript, including output that is still changing. Native terminal scrollback is not the active transcript. Streaming and completed tools keep their ordinary grouped presentation; finalization changes cache eligibility without appending a duplicate card.

The logical document keeps the existing component ownership and order. By default its above-editor widgets, current editor-slot occupant, below-editor widgets and footer form a bottom dock; header, chat, queue and working status remain scrollable. Live `/settings` controls docking, maximum input-text height and wheel step. The percentage is a text-row ceiling, not reserved space or a selector-height limit; the editor fits around the other dock occupants before the renderer suspends docking with a visible explanation. Oversized content remains in the retained scrolling flow. Tool-attached approval context and controls stay together in the transcript, with dock space reclaimed while they own input. Undocking preserves component identity and does not switch renderer.

Passive updates, docked editing and configured presentation toggles preserve the reading position. Undocked editing reveals its cursor. Overlays remain screen-relative. Session reconstruction resets browsing state, but ordinary updates, expansion, styling, docking and resize attempt content-relative anchoring.

See [terminal-setup.md](terminal-setup.md#transcript-browsing-and-copying) for interaction and terminal-configuration requirements. Session data, tool result contracts, RPC/print output and standalone HTML export are unchanged; viewport state is not serialized.

### Retained viewport API

`TUI.configureViewport({ getBlocks, keybindings?, copy?, requestPaste?, getScrollWheelStep?, handlePresentationInput?, keepReadingOnInput?, allowViewportKeys?, minimumSize?, handleBlockedInput? })` selects the retained rendering path. InteractiveMode configures it at startup using the mounted production components; standalone TUI callers opt in explicitly. Only promoted committed components can skip producer rendering; completed tools behind a pending predecessor remain live and browseable. Live producers still render, but their normalized rows and copy spans can be reused when raw output, copy provenance and geometry remain unchanged. Promotion preserves component identity. Thinking visibility and expansion update existing components; session/tree/reload reconstruction resets browsing and selection. `getBlocks()` returns the ordered projection of existing component instances as `{ component, finalized?, revision?, dock?, fitHeight? }`. Each component must occur once, and dock-tagged blocks must form a suffix. Fixed dock occupants are measured before `fitHeight(rows)` assigns a remaining row ceiling to the input slot; an oversized band uses the scrolling flow instead. The callback does not authorize clipping arbitrary text. `EditorComponent.setHeightLimit?(limits)` is a separate optional input-sizing hook: `limits()` returns `{ rows, text }`, the current total editor budget and wrapped-text ceiling. Built-in editors use it for cursor-following and page movement and budget autocomplete separately. Optional `EditorComponent.isShowingAutocomplete()` exposes an active completion menu even when it hides the hardware cursor. InteractiveMode uses this capability to reveal a hidden input slot and gate completion acceptance until its current visible paint has flushed, without taking input from capturing overlays. Completion acceptance also checks the text/cursor snapshot that produced the displayed menu, independently of viewport visibility and flush evidence. If that snapshot is stale, Tab requests fresh completion; Enter follows normal handling of the current draft rather than applying the stale prefix, and confirmation-only remaps cannot fall through to unrelated editor actions. Custom editors must cooperate to honor the input-height preference; non-fitting custom components are not silently clipped vertically. The existing `Component.setViewportHeight` contract remains image-only. Finalization changes cache eligibility, not identity or transcript ownership. Increment `revision` when a finalized block changes; `tui.invalidate()` invalidates projected components and finalization-based reuse. Width changes and `rebuild()` refresh rendering without resetting the reader's anchor; unchanged live output may still reuse its verified normalization. Call `resetViewport()` only for genuine content/session replacement.

The viewport retains all logical rows, renders components at terminal width minus one, and paints a bounded slice using absolute screen coordinates. The reserved last column displays a scrollbar; at a one-column terminal there is no scrollbar. `scrollViewport(delta)` and `scrollViewportTo(offset, anchorScreenRow = 0)` provide programmatic navigation. `followViewport()` releases held selection and resumes tail-following; InteractiveMode invokes it when a new user message appears, including queued messages when delivered. Scrolling to the bottom resumes tail-following; passive output updates and resize clamping do not. `getViewportState()` returns the scrollable transcript's offset, total rows, visible height, and tail-following state, excluding a fitted dock; selection does not reserve a row. `refreshViewportLayout()` clears held selection/gestures and invalidates layout without resetting the reader's anchor; use it for explicit layout-setting changes. Overlays remain screen-relative, and cursor/IME positioning uses the visible slice rather than the document tail.

Ordinary image-free frames without held selection assemble visible transcript and dock ranges directly from cached blocks, rather than flattening every historical row. Cursor revelation uses cached per-block marker positions. Image/cursor summaries follow the same normalized-row invalidation as the block cache. This does not virtualize producers: block traversal and necessary live rendering/provenance checks remain, and changed blocks are still validated completely, including offscreen rows. Image-bearing presentations retain the full-document graphics path because an image's payload row need not be its placement top. Active selection uses the full-document slicing path. Each paint records its immutable row/provenance references and endpoint values; live producers and geometry continue updating. All logical rows remain retained and reachable, with no history cap.

Anchors use ordered row correspondence with a bounded raw-row search, then visible grapheme correspondence across reflow and restyling. Exhausting the raw-row search still attempts content correspondence; it is not evidence of deletion. Comparison ignores ANSI, whitespace, and wrap boundaries without altering displayed rows or their spacing. The anchored grapheme retains its chosen screen-row offset where geometry permits. Deletion falls back to surviving content in the same block, then the nearest surviving adjacent block (following block wins ties). Blank-only blocks preserve a clamped row ordinal; arbitrary width-dependent or repeated/whitespace-only content cannot promise exact semantic source identity.

`renderNow({ requireFlush: true })` renders immediately and rejects if the terminal cannot flush or flushing fails. It waits for that invocation's output, not necessarily a newer frame rendered during the wait. `isViewportFrameFlushed()` checks whether the current complete viewport paint, including overlays, has a matching successful flush; an older flush cannot certify a newer presentation. Scheduled and immediate paints share this settlement path. Failed scheduled flushes leave controls gated and produce a bounded diagnostic; immediate callers still receive the original rejection. Stop, reset and rebuild revoke previous paint evidence. It is independent of `commitNow()`, whose committed-history preconditions and flush guarantee remain unchanged. Viewport and committed live-region configuration are mutually exclusive.

Retained ordinary text normalizes terminal presentation and expands tabs to three spaces before width containment. Overwide component rows are clipped with a bounded diagnostic rather than crashing the session; caching, selection and copying use the same contained rows. Source messages, tool results and HTML export remain unchanged, and graphics retain separate atomic placement handling. Components must still obey the width contract. `isComponentRenderComplete(component)` checks the entire projected block, including offscreen rows, and returns false for clipping, unavailable or invalidated presentation, or a held selection. Completeness is separate from visible control geometry and flush settlement.

The viewport owns transcript wheel scrolling, track clicks and thumb dragging. `getScrollWheelStep()` supplies the application's validated step. Thumb mapping stays fixed during a gesture, then reconciles with current bounds; releasing at the gesture's bottom resumes following the current tail after growth. Configurable `tui.viewport.pageUp/pageDown` default to Alt+PageUp/Alt+PageDown and allow entry from the tail; `allowViewportKeys(data)` lets the application preserve conflicting selector bindings. PageUp/PageDown/Escape retain their fixed detached-only browsing behavior when the input owner permits it. Ctrl+Home/Ctrl+End navigate to the transcript beginning/bottom at every scroll position; Home/End retain editor line-start/end behavior. Selector bindings, including cancellation, take precedence. `keepReadingOnInput()` preserves position for input in a fitted dock, while `handlePresentationInput(data)` optionally dispatches application-owned presentation toggles without reattaching. These callbacks do not supersede overlay or selection-copy precedence. Ordinary undocked input reveals its cursor marker on the next frame, even with tall trailing widgets. Later viewport-consumed input or programmatic navigation cancels pending cursor and component revelation, including hidden-completion revelation. Visible overlays cancel underlying selection and gestures. Raw-input listeners registered before configuration still run first; do not transform or consume pointer packets if the viewport must own them. Interactive extension listeners are registered after viewport configuration and do not receive consumed pointer/browsing events. Kitty-capable viewport terminals request flags 15 (including explicit escape encoding for all keys), so raw listeners must use the key parser and ignore release events rather than assuming Enter is a bare carriage return. This prevents older Kitty versions' legacy Enter release bytes from looking like a second activation. Pure modifier presses are consumed by the viewport without moving the reader or clearing selection, so forming a copy chord preserves its target. Keyboard stacks are restored in their owning screen buffer; unconfigured terminals retain flags 7.

Ordinary drag selection snaps to displayed grapheme boundaries and copies selected displayed content without ANSI controls or scrollbar cells. Built-in rendering supplies optional copy provenance to exclude layout padding and rejoin soft wraps without guessing at source indentation or hard breaks. Selection can cross the transcript/dock seam in either direction. Downward transcript dragging first autoscrolls to the tail before extending into the dock, including with a stationary pointer. Dock-origin upward dragging captures the visible transcript boundary, excluding the hidden gap below it. Cross-seam selection retains a visual boundary rather than extending that boundary through newly inserted offscreen rows. Transcript selection supports edge autoscroll and, after an initial drag, wheel-driven extension while the left button stays held, including with a stationary pointer. Initial press and pre-scroll motion target the painted rows. Screen-only filler below a short transcript cannot start selection; genuine blank content rows remain selectable. Once wheel or edge scrolling begins, motion and wheel endpoints use the resulting offset and height even when events interleave before repaint; wheel coordinates also refresh the edge timer's pointer position. Horizontal adjustments keep displayed grapheme boundaries. A press without a drag does not start wheel selection; releasing the button stops pointer-driven extension. Wheel browsing does not extend a selection confined to the dock. Selection follows surviving component-local rows and columns on a best-effort basis during replacement, deletion and reflow; layout and waiting controls remain live. Copy captures the last painted highlight and its copy provenance synchronously before invoking the asynchronous clipboard backend. Pending pointer movement or producer updates cannot alter that captured text. A new, not-yet-painted selection consumes Copy without copying or falling through to editor clearing. Copy failure temporarily covers the last screen row with an actionable diagnostic, retaining selection and geometry; that painted diagnostic cannot select the underlying text, even before its dismissal repaints. Successful copy, clearing input, reset or stop clears selection; usable resize ends the gesture and remaps selection. Successful copy and empty-click release restore prior following intent unless explicit navigation moved away; passive output growth does not discard that intent. Generic cancellation does not restore following. Right-click with a nonempty selection and the injected manager's `tui.input.copy` action invoke `copy(text): Promise<void>`; absent selection, Ctrl+C keeps its normal behavior. Copy failures are displayed and retain the selection. Callback resolution means the backend accepted the request, not that the desktop clipboard was independently verified; OSC 52 has no such acknowledgement. Without selection, right-click hit-tests a projected block and invokes optional `requestPaste(component)` only for a visible, complete, flushed presentation without overlays. InteractiveMode accepts only its actual focused editor, never transcript/widgets/footer/selectors or approval controls. It reads the local clipboard with bounded platform commands, rejects stale input/edit/slot/session results, strips terminal framing/control bytes, and inserts through bracketed paste without submission. Clipboard copy remains single-flight even if its originating selection is replaced; stale settlement cannot clear a replacement selection. Emulator-owned menus cannot see application selection, and native clipboard/interaction checks remain necessary for each supported path.

Copy provenance uses the unchanged `Component.render(width): string[]` contract. `setRenderedCopy(lines, rows)` validates and associates an immutable metadata snapshot with that exact returned array; equivalent publication on unchanged output reuses the verified snapshot. `getRenderedCopy(lines)` returns it only while the output still matches its witness, otherwise falling back to physical-cell copying. Component caches verify both output and copy provenance, including metadata-only changes and externally rebound output. Each `RenderedCopyRow` is either `null` for layout padding or `{ start, end, after? }`, using half-open display-column bounds. `after` records only the whitespace replaced by a soft wrap (`""` for a midword split); absent `after` means a hard line boundary. Metadata never supplies alternate non-whitespace content. Text, Markdown, Box and Container propagate it through wrapping and padding; known zero-width OSC wrappers reattach it after their transformation. Cached and painted viewport rows retain their own copy snapshot, independent of later producer updates. The editor annotates visible draft chunks using logical-line boundaries, excluding borders, padding and the synthetic cursor space; hidden draft rows are not recovered. Custom wrappers that mutate or reconstruct arrays must deliberately propagate correct metadata or retain the conservative fallback. Tables and repeated non-whitespace quote prefixes keep their physical structure, and pre-render trimming/collapsing is not reversed. Copying a padding-only held range does not fall through to editor clearing.

`ProcessTerminal` implements the additive `Terminal.setViewportMode(enabled)` capability, required and checked before viewport configuration. Injected InteractiveMode terminals must implement this capability; there is no silent fallback to the clipping renderer. TUI start/stop coordinates idempotent alternate-screen, SGR button-motion and focus-reporting mode entry/restoration; unconfigured callers never enable these modes. Focus packets are consumed before ordinary input. Focus-out cancels unfinished selection/scrollbar gestures without returning to the tail, preserves completed selections, and stops edge autoscroll. Recognized releases terminate gestures even outside screen bounds; boundary scrolling does not keep repainting unchanged rows. Focus-in does not revive an abandoned gesture. These guarantees depend on the terminal delivering focus reports. Recognized incomplete mouse packets are quarantined after the transport timeout and discarded through a terminator or next escape, rather than leaking their tails into editor text. While mouse reporting is enabled, `StdinBuffer.setMouseReporting(true)` also retains one recognition credit after emitting an ambiguous Escape/CSI prefix. A subsequent complete SGR-shaped suffix is discarded even after a long idle gap; it does not replay the Escape or trigger a late pointer action. Nonmatching new bytes are replayed literally, and incomplete speculative suffixes are released after the framing timeout (10 ms in ProcessTerminal). Identical independently typed mouse-shaped text is inherently ambiguous and is consumed in this one-credit position; a suffix fragmented beyond the timeout can still become literal input. Unrelated input, paste, mode exit and reset clear the credit; recognized `ESC[<` quarantine remains separate. Standalone callers without mouse reporting retain the shorter prefix-recovery window. Headless `sendInput` bypasses transport framing and cannot establish fragmentation safety.

Tool-attached context uses `renderNow({ requireFlush: true })` after installing its full retained context and revealing the pending tool. Controls receive focus only after a successful required flush matches the current presentation and both the full context and controls are render-complete. Missing/failed flushing, replaced context, clipped context (even offscreen), or controls that cannot fit wholly in the viewport fail closed. If a newer presentation supersedes a pending flush, controls remain unfocused until a later reveal/flush establishes current evidence; the older completion cannot authorize the newer frame. While browsing away, the first non-browsing input reveals and flushes controls without activating them; only subsequent input can authorize. `isComponentVisible(component)` checks the last painted, fully visible projected block (not a scheduled scroll); `revealComponent(component)` requests its revelation on the next frame. `getComponentVisibility(component)` distinguishes `visible`, `occluded` (a current or last-painted overlay), and `outside` (non-fit, stale geometry or stopped rendering). Temporary overlay occlusion does not cancel approval: after dismissal, controls require fresh revelation and flushing before a later activation. These visibility methods concern projected blocks, not arbitrary nested children. `isComponentFocused(component)` identifies the actual input owner so approval guards do not steal input from a capturing overlay. `replaceFocus(expected, next)` conditionally updates both actual focus and saved overlay restoration references matching `expected`, preserving other focused overlays and newer targets across asynchronous settlement/cancellation.

`ui.stop()` is a temporary handoff: it restores the normal buffer and input modes without printing transcript copies. Interactive suspension and both external-editor paths first await the existing bounded input drain, so the opening key's release does not leak to the shell or editor. Late keyboard-query responses cannot re-enable reporting during that drain. Restart restores the retained view independently of the asynchronous keyboard-protocol query. InteractiveMode's final `stop()` additionally passes the current chat components to `ui.stop({ transcript })`, appending one readable plain-text presentation to the restored normal buffer, without clearing prior shell output. Editor, widgets and attached controls are excluded; native images become `[Image]` labels. Repeated stop does not append again. Orderly shutdown awaits terminal flushing; crash cleanup restores modes without promising a transcript, and terminal-loss cleanup deliberately writes nothing.

Built-in Kitty/iTerm2 placements are re-anchored to absolute rows only when their full extent fits; clipped spans show a placeholder and remain reachable by scrolling. Repeated reports of unchanged cell dimensions are consumed without invalidating components or disturbing image reading anchors; genuine dimension changes still invalidate and re-render images. `Component.setViewportHeight?(height)` is an optional image-sizing bound, propagated by standard Container/Box rendering, including newly added children. Custom wrappers must forward it to nested images. It must not truncate ordinary text or bound the total stacked document. Built-in Image fits using existing aspect-preserving sizing without changing source data; finalized viewport caches refresh on height changes. iTerm2 placements specify both cell dimensions to avoid rounded width exceeding reserved height. Transcript images are withheld with visible placeholders during overlays and selection; clearing either restores eligible placements. Bounded overlays forward their resolved image-height limit before rendering and use the same atomic geometry validation before clipping, including nested/padded images. A higher overlapping overlay suppresses the entire lower image placement; clearing it restores eligible placements. Custom graphics envelopes without recognizable placement geometry cannot be safely clipped and receive a placeholder. Built-in graphics checks do not establish arbitrary custom-envelope compatibility. See [terminal-setup.md](terminal-setup.md#native-compatibility-evidence) for the bounded native evidence.

### Minimum usable geometry

InteractiveMode opts into `minimumSize: { columns: 12, rows: 3 }`, measured in physical terminal cells. Below that threshold the retained renderer shows a bounded resize notice, suppresses overlays/images/cursors and ordinary input, and does not render components into a destructive temporary budget. Draft and autocomplete state survive recovery. Live dimensions guard shrink-before-paint; a painted resize notice keeps input blocked until a usable replacement paint has flushed. This opt-in protection requires terminal flushing; standalone callers without `minimumSize` retain their existing small-screen behavior.

Focus and cell-size responses still reach their protocol owners. Releases, pointer packets and bracketed paste cannot activate hidden controls or invoke `handleBlockedInput`. InteractiveMode uses that callback only for configured interrupt/empty-draft exit and safe settings cancellation; interrupt cancels active agent, bash, compaction and branch-summary operations rather than invoking hidden component handlers. A whitespace-only draft is not empty. The cutoff is an emergency bound, not a guarantee that every custom control fits above it.

### Standalone legacy and committed modes

Unconfigured TUI callers retain legacy rendering. `setLiveRegionStart(component)` instead selects an append-only committed prefix and a tail-windowed live canvas, with `commit()` scheduling promotion and `commitNow({ requireFlush: true })` requiring successful flush settlement. Detaching that direct child returns to legacy rendering. These public modes remain separate from InteractiveMode's viewport; their native-history behavior has not been redefined. `rebuild()` deliberately repaints their retained history and can bottom-anchor it.

## Component Interface

All components implement:

```typescript
interface Component {
  render(width: number): string[];
  setViewportHeight?(height: number | undefined): void;
  handleInput?(data: string): void;
  wantsKeyRelease?: boolean;
  invalidate(): void;
}
```

| Method | Description |
|--------|-------------|
| `render(width)` | Return array of strings (one per line). Each line **must not exceed `width`**. |
| `setViewportHeight?(height)` | Optional image-sizing bound. Forward through custom image wrappers; never truncate ordinary logical text. |
| `handleInput?(data)` | Receive keyboard input when component has focus. Use key parsers, not raw-byte equality. |
| `wantsKeyRelease?` | If true, component receives key release events (Kitty protocol). Default: false. |
| `invalidate()` | Clear cached render state. Called on theme changes. |

The TUI appends a full SGR reset and OSC 8 reset at the end of each rendered line. Styles do not carry across lines. If you emit multi-line text with styling, reapply styles per line or use `wrapTextWithAnsi()` so styles are preserved for each wrapped line.

## Focusable Interface (IME Support)

Components that display a text cursor and need IME (Input Method Editor) support should implement the `Focusable` interface:

```typescript
import { CURSOR_MARKER, type Component, type Focusable } from "@leanandmean/tui";

class MyInput implements Component, Focusable {
  focused: boolean = false;  // Set by TUI when focus changes

  render(width: number): string[] {
    const marker = this.focused ? CURSOR_MARKER : "";
    // Emit marker right before the fake cursor
    return [`> ${beforeCursor}${marker}\x1b[7m${atCursor}\x1b[27m${afterCursor}`];
  }
}
```

When a `Focusable` component has focus, TUI:
1. Sets `focused = true` on the component
2. Scans rendered output for `CURSOR_MARKER` (a zero-width APC escape sequence)
3. Positions the hardware terminal cursor at that location
4. Shows the hardware cursor

This enables IME candidate windows to appear at the correct position for CJK input methods. The `Editor` and `Input` built-in components already implement this interface.

### Container Components with Embedded Inputs

When a container component (dialog, selector, etc.) contains an `Input` or `Editor` child, the container must implement `Focusable` and propagate the focus state to the child. Otherwise, the hardware cursor won't be positioned correctly for IME input.

```typescript
import { Container, type Focusable, Input } from "@leanandmean/tui";

class SearchDialog extends Container implements Focusable {
  private searchInput: Input;

  // Focusable implementation - propagate to child input for IME cursor positioning
  private _focused = false;
  get focused(): boolean {
    return this._focused;
  }
  set focused(value: boolean) {
    this._focused = value;
    this.searchInput.focused = value;
  }

  constructor() {
    super();
    this.searchInput = new Input();
    this.addChild(this.searchInput);
  }
}
```

Without this propagation, typing with an IME (Chinese, Japanese, Korean, etc.) will show the candidate window in the wrong position on screen.

## Using Components

**In extensions** via `ctx.ui.custom()`:

```typescript
pi.on("session_start", async (_event, ctx) => {
  const handle = ctx.ui.custom(myComponent);
  // handle.requestRender() - trigger re-render
  // handle.close() - restore normal UI
});
```

**In custom tools** via `ctx.ui.custom()`:

```typescript
async execute(toolCallId, params, onUpdate, ctx, signal) {
  const result = await ctx.ui.custom((_tui, _theme, _keybindings, done) => myComponent(done));
  // ...
}
```

### Tool-attached retained context

A pending sequential tool can pair compact live controls with immutable long-form context retained at that tool's transcript position:

```typescript
const result = await ctx.ui.custom(
  (_tui, _theme, _keybindings, done) => new ApprovalSelector(done),
  {
    toolAttachedContext: {
      toolCallId,
      render: (_tui, theme) => new Markdown(completePayload, 0, 0, getMarkdownTheme()),
    },
  },
);
```

The context is constructed after the controls factory resolves and installed completely at the named pending tool row. It remains browseable even when taller than the screen; it need not all be simultaneously visible or written to native scrollback. The editor is defocused, controls are revealed, and a required visible-frame flush must settle before controls receive focus. Browsing away consumes the first activation to reveal and flush them; only a subsequent activation can authorize. Missing/non-leading/settled rows, replaced context, missing or failed flushes, and controls too tall to fit fail closed. The context is visual-only, not persisted or sent to the model. Use this only from the sequential tool identified by `toolCallId`.

## Overlays

Overlays render components on top of existing content without clearing the screen. Pass `{ overlay: true }` to `ctx.ui.custom()`:

```typescript
const result = await ctx.ui.custom<string | null>(
  (tui, theme, keybindings, done) => new MyDialog({ onClose: done }),
  { overlay: true }
);
```

For positioning and sizing, use `overlayOptions`:

```typescript
const result = await ctx.ui.custom<string | null>(
  (tui, theme, keybindings, done) => new SidePanel({ onClose: done }),
  {
    overlay: true,
    overlayOptions: {
      // Size: number or percentage string
      width: "50%",          // 50% of terminal width
      minWidth: 40,          // minimum 40 columns
      maxHeight: "80%",      // max 80% of terminal height

      // Position: anchor-based (default: "center")
      anchor: "right-center", // 9 positions: center, top-left, top-center, etc.
      offsetX: -2,            // offset from anchor
      offsetY: 0,

      // Or percentage/absolute positioning
      row: "25%",            // 25% from top
      col: 10,               // column 10

      // Margins
      margin: 2,             // all sides, or { top, right, bottom, left }

      // Responsive: hide on narrow terminals
      visible: (termWidth, termHeight) => termWidth >= 80,
    },
    // Get handle for programmatic visibility control
    onHandle: (handle) => {
      // handle.setHidden(true/false) - toggle visibility
      // handle.hide() - permanently remove
    },
  }
);
```

### Overlay Lifecycle

Overlay components are disposed when closed. Don't reuse references - create fresh instances:

```typescript
// Wrong - stale reference
let menu: MenuComponent;
await ctx.ui.custom((_, __, ___, done) => {
  menu = new MenuComponent(done);
  return menu;
}, { overlay: true });
setActiveComponent(menu);  // Disposed

// Correct - re-call to re-show
const showMenu = () => ctx.ui.custom((_, __, ___, done) =>
  new MenuComponent(done), { overlay: true });

await showMenu();  // First show
await showMenu();  // "Back" = just call again
```

See [overlay-qa-tests.ts](../examples/extensions/overlay-qa-tests.ts) for comprehensive examples covering anchors, margins, stacking, responsive visibility, and animation.

## Built-in Components

Import from `@leanandmean/tui`:

```typescript
import { Text, Box, Container, Spacer, Markdown } from "@leanandmean/tui";
```

### Text

Multi-line text with word wrapping.

```typescript
const text = new Text(
  "Hello World",    // content
  1,                // paddingX (default: 1)
  1,                // paddingY (default: 1)
  (s) => bgGray(s)  // optional background function
);
text.setText("Updated");
```

### Box

Container with padding and background color.

```typescript
const box = new Box(
  1,                // paddingX
  1,                // paddingY
  (s) => bgGray(s)  // background function
);
box.addChild(new Text("Content", 0, 0));
box.setBgFn((s) => bgBlue(s));
```

### Container

Groups child components vertically.

```typescript
const container = new Container();
container.addChild(component1);
container.addChild(component2);
container.removeChild(component1);
```

### Spacer

Empty vertical space.

```typescript
const spacer = new Spacer(2);  // 2 empty lines
```

### Markdown

Renders markdown with syntax highlighting.

```typescript
const md = new Markdown(
  "# Title\n\nSome **bold** text",
  1,        // paddingX
  1,        // paddingY
  theme     // MarkdownTheme (see below)
);
md.setText("Updated markdown");
```

### Image

Renders images in supported terminals (Kitty, iTerm2, Ghostty, WezTerm).

```typescript
const image = new Image(
  base64Data,   // base64-encoded image
  "image/png",  // MIME type
  theme,        // ImageTheme
  { maxWidthCells: 80, maxHeightCells: 24 }
);
```

## Keyboard Input

Use `matchesKey()` for key detection:

```typescript
import { matchesKey, Key } from "@leanandmean/tui";

handleInput(data: string) {
  if (matchesKey(data, Key.up)) {
    this.selectedIndex--;
  } else if (matchesKey(data, Key.enter)) {
    this.onSelect?.(this.selectedIndex);
  } else if (matchesKey(data, Key.escape)) {
    this.onCancel?.();
  } else if (matchesKey(data, Key.ctrl("c"))) {
    // Ctrl+C
  }
}
```

**Key identifiers** (use `Key.*` for autocomplete, or string literals):
- Basic keys: `Key.enter`, `Key.escape`, `Key.tab`, `Key.space`, `Key.backspace`, `Key.delete`, `Key.home`, `Key.end`
- Arrow keys: `Key.up`, `Key.down`, `Key.left`, `Key.right`
- With modifiers: `Key.ctrl("c")`, `Key.shift("tab")`, `Key.alt("left")`, `Key.ctrlShift("p")`
- String format also works: `"enter"`, `"ctrl+c"`, `"shift+tab"`, `"ctrl+shift+p"`

## Line Width

**Critical:** Each line from `render()` must not exceed the `width` parameter.

```typescript
import { visibleWidth, truncateToWidth } from "@leanandmean/tui";

render(width: number): string[] {
  // Truncate long lines
  return [truncateToWidth(this.text, width)];
}
```

Utilities:
- `visibleWidth(str)` - Get display width (ignores ANSI codes)
- `truncateToWidth(str, width, ellipsis?)` - Truncate with optional ellipsis
- `wrapTextWithAnsi(str, width)` - Word wrap preserving ANSI codes

## Creating Custom Components

Example: Interactive selector

```typescript
import {
  matchesKey, Key,
  truncateToWidth, visibleWidth
} from "@leanandmean/tui";

class MySelector {
  private items: string[];
  private selected = 0;
  private cachedWidth?: number;
  private cachedLines?: string[];

  public onSelect?: (item: string) => void;
  public onCancel?: () => void;

  constructor(items: string[]) {
    this.items = items;
  }

  handleInput(data: string): void {
    if (matchesKey(data, Key.up) && this.selected > 0) {
      this.selected--;
      this.invalidate();
    } else if (matchesKey(data, Key.down) && this.selected < this.items.length - 1) {
      this.selected++;
      this.invalidate();
    } else if (matchesKey(data, Key.enter)) {
      this.onSelect?.(this.items[this.selected]);
    } else if (matchesKey(data, Key.escape)) {
      this.onCancel?.();
    }
  }

  render(width: number): string[] {
    if (this.cachedLines && this.cachedWidth === width) {
      return this.cachedLines;
    }

    this.cachedLines = this.items.map((item, i) => {
      const prefix = i === this.selected ? "> " : "  ";
      return truncateToWidth(prefix + item, width);
    });
    this.cachedWidth = width;
    return this.cachedLines;
  }

  invalidate(): void {
    this.cachedWidth = undefined;
    this.cachedLines = undefined;
  }
}
```

Usage in an extension:

```typescript
pi.registerCommand("pick", {
  description: "Pick an item",
  handler: async (args, ctx) => {
    const items = ["Option A", "Option B", "Option C"];
    const selector = new MySelector(items);

    let handle: { close: () => void; requestRender: () => void };

    await new Promise<void>((resolve) => {
      selector.onSelect = (item) => {
        ctx.ui.notify(`Selected: ${item}`, "info");
        handle.close();
        resolve();
      };
      selector.onCancel = () => {
        handle.close();
        resolve();
      };
      handle = ctx.ui.custom(selector);
    });
  }
});
```

## Theming

Components accept theme objects for styling.

**In `renderCall`/`renderResult`**, use the `theme` parameter:

```typescript
renderResult(result, options, theme, context) {
  // Use theme.fg() for foreground colors
  return new Text(theme.fg("success", "Done!"), 0, 0);

  // Use theme.bg() for background colors
  const styled = theme.bg("toolPendingBg", theme.fg("accent", "text"));
}
```

**Foreground colors** (`theme.fg(color, text)`):

| Category | Colors |
|----------|--------|
| General | `text`, `accent`, `muted`, `dim` |
| Status | `success`, `error`, `warning` |
| Borders | `border`, `borderAccent`, `borderMuted` |
| Messages | `userMessageText`, `customMessageText`, `customMessageLabel` |
| Tools | `toolTitle`, `toolOutput` |
| Diffs | `toolDiffAdded`, `toolDiffRemoved`, `toolDiffContext` |
| Markdown | `mdHeading`, `mdLink`, `mdLinkUrl`, `mdCode`, `mdCodeBlock`, `mdCodeBlockBorder`, `mdQuote`, `mdQuoteBorder`, `mdHr`, `mdListBullet` |
| Syntax | `syntaxComment`, `syntaxKeyword`, `syntaxFunction`, `syntaxVariable`, `syntaxString`, `syntaxNumber`, `syntaxType`, `syntaxOperator`, `syntaxPunctuation` |
| Thinking | `thinkingOff`, `thinkingMinimal`, `thinkingLow`, `thinkingMedium`, `thinkingHigh`, `thinkingXhigh` |
| Modes | `bashMode` |

**Background colors** (`theme.bg(color, text)`):

`selectedBg`, `userMessageBg`, `customMessageBg`, `toolPendingBg`, `toolSuccessBg`, `toolErrorBg`

**For Markdown**, use `getMarkdownTheme()`:

```typescript
import { getMarkdownTheme } from "@leanandmean/coding-agent";
import { Markdown } from "@leanandmean/tui";

renderResult(result, options, theme, context) {
  const mdTheme = getMarkdownTheme();
  return new Markdown(result.details.markdown, 0, 0, mdTheme);
}
```

**For custom components**, define your own theme interface:

```typescript
interface MyTheme {
  selected: (s: string) => string;
  normal: (s: string) => string;
}
```

## Debug logging

Set `PI_TUI_WRITE_LOG` to capture the raw ANSI stream written to stdout.

```bash
PI_TUI_WRITE_LOG=/tmp/tui-ansi.log npx tsx packages/tui/test/chat-simple.ts
```

## Performance

Cache rendered output when possible:

```typescript
class CachedComponent {
  private cachedWidth?: number;
  private cachedLines?: string[];

  render(width: number): string[] {
    if (this.cachedLines && this.cachedWidth === width) {
      return this.cachedLines;
    }
    // ... compute lines ...
    this.cachedWidth = width;
    this.cachedLines = lines;
    return lines;
  }

  invalidate(): void {
    this.cachedWidth = undefined;
    this.cachedLines = undefined;
  }
}
```

Call `invalidate()` when state changes, then `handle.requestRender()` to trigger re-render.

## Invalidation and Theme Changes

When the theme changes, the TUI calls `invalidate()` on all components to clear their caches. Components must properly implement `invalidate()` to ensure theme changes take effect.

### The Problem

If a component pre-bakes theme colors into strings (via `theme.fg()`, `theme.bg()`, etc.) and caches them, the cached strings contain ANSI escape codes from the old theme. Simply clearing the render cache isn't enough if the component stores the themed content separately.

**Wrong approach** (theme colors won't update):

```typescript
class BadComponent extends Container {
  private content: Text;

  constructor(message: string, theme: Theme) {
    super();
    // Pre-baked theme colors stored in Text component
    this.content = new Text(theme.fg("accent", message), 1, 0);
    this.addChild(this.content);
  }
  // No invalidate override - parent's invalidate only clears
  // child render caches, not the pre-baked content
}
```

### The Solution

Components that build content with theme colors must rebuild that content when `invalidate()` is called:

```typescript
class GoodComponent extends Container {
  private message: string;
  private content: Text;

  constructor(message: string) {
    super();
    this.message = message;
    this.content = new Text("", 1, 0);
    this.addChild(this.content);
    this.updateDisplay();
  }

  private updateDisplay(): void {
    // Rebuild content with current theme
    this.content.setText(theme.fg("accent", this.message));
  }

  override invalidate(): void {
    super.invalidate();  // Clear child caches
    this.updateDisplay(); // Rebuild with new theme
  }
}
```

### Pattern: Rebuild on Invalidate

For components with complex content:

```typescript
class ComplexComponent extends Container {
  private data: SomeData;

  constructor(data: SomeData) {
    super();
    this.data = data;
    this.rebuild();
  }

  private rebuild(): void {
    this.clear();  // Remove all children

    // Build UI with current theme
    this.addChild(new Text(theme.fg("accent", theme.bold("Title")), 1, 0));
    this.addChild(new Spacer(1));

    for (const item of this.data.items) {
      const color = item.active ? "success" : "muted";
      this.addChild(new Text(theme.fg(color, item.label), 1, 0));
    }
  }

  override invalidate(): void {
    super.invalidate();
    this.rebuild();
  }
}
```

### When This Matters

This pattern is needed when:

1. **Pre-baking theme colors** - Using `theme.fg()` or `theme.bg()` to create styled strings stored in child components
2. **Syntax highlighting** - Using `highlightCode()` which applies theme-based syntax colors
3. **Complex layouts** - Building child component trees that embed theme colors

This pattern is NOT needed when:

1. **Using theme callbacks** - Passing functions like `(text) => theme.fg("accent", text)` that are called during render
2. **Simple containers** - Just grouping other components without adding themed content
3. **Stateless render** - Computing themed output fresh in every `render()` call (no caching)

## Common Patterns

These patterns cover the most common UI needs in extensions. **Copy these patterns instead of building from scratch.**

### Pattern 1: Selection Dialog (SelectList)

For letting users pick from a list of options. Use `SelectList` from `@leanandmean/tui` with `DynamicBorder` for framing.

```typescript
import type { ExtensionAPI } from "@leanandmean/coding-agent";
import { DynamicBorder } from "@leanandmean/coding-agent";
import { Container, type SelectItem, SelectList, Text } from "@leanandmean/tui";

pi.registerCommand("pick", {
  handler: async (_args, ctx) => {
    const items: SelectItem[] = [
      { value: "opt1", label: "Option 1", description: "First option" },
      { value: "opt2", label: "Option 2", description: "Second option" },
      { value: "opt3", label: "Option 3" },  // description is optional
    ];

    const result = await ctx.ui.custom<string | null>((tui, theme, _kb, done) => {
      const container = new Container();

      // Top border
      container.addChild(new DynamicBorder((s: string) => theme.fg("accent", s)));

      // Title
      container.addChild(new Text(theme.fg("accent", theme.bold("Pick an Option")), 1, 0));

      // SelectList with theme
      const selectList = new SelectList(items, Math.min(items.length, 10), {
        selectedPrefix: (t) => theme.fg("accent", t),
        selectedText: (t) => theme.fg("accent", t),
        description: (t) => theme.fg("muted", t),
        scrollInfo: (t) => theme.fg("dim", t),
        noMatch: (t) => theme.fg("warning", t),
      });
      selectList.onSelect = (item) => done(item.value);
      selectList.onCancel = () => done(null);
      container.addChild(selectList);

      // Help text
      container.addChild(new Text(theme.fg("dim", "↑↓ navigate • enter select • esc cancel"), 1, 0));

      // Bottom border
      container.addChild(new DynamicBorder((s: string) => theme.fg("accent", s)));

      return {
        render: (w) => container.render(w),
        invalidate: () => container.invalidate(),
        handleInput: (data) => { selectList.handleInput(data); tui.requestRender(); },
      };
    });

    if (result) {
      ctx.ui.notify(`Selected: ${result}`, "info");
    }
  },
});
```

**Examples:** [preset.ts](../examples/extensions/preset.ts), [tools.ts](../examples/extensions/tools.ts)

### Pattern 2: Async Operation with Cancel (BorderedLoader)

For operations that take time and should be cancellable. `BorderedLoader` shows a spinner and handles escape to cancel.

```typescript
import { BorderedLoader } from "@leanandmean/coding-agent";

pi.registerCommand("fetch", {
  handler: async (_args, ctx) => {
    const result = await ctx.ui.custom<string | null>((tui, theme, _kb, done) => {
      const loader = new BorderedLoader(tui, theme, "Fetching data...");
      loader.onAbort = () => done(null);

      // Do async work
      fetchData(loader.signal)
        .then((data) => done(data))
        .catch(() => done(null));

      return loader;
    });

    if (result === null) {
      ctx.ui.notify("Cancelled", "info");
    } else {
      ctx.ui.setEditorText(result);
    }
  },
});
```

**Examples:** [qna.ts](../examples/extensions/qna.ts), [handoff.ts](../examples/extensions/handoff.ts)

### Pattern 3: Settings/Toggles (SettingsList)

For toggling multiple settings. Use `SettingsList` from `@leanandmean/tui` with `getSettingsListTheme()`.

```typescript
import { getSettingsListTheme } from "@leanandmean/coding-agent";
import { Container, type SettingItem, SettingsList, Text } from "@leanandmean/tui";

pi.registerCommand("settings", {
  handler: async (_args, ctx) => {
    const items: SettingItem[] = [
      { id: "verbose", label: "Verbose mode", currentValue: "off", values: ["on", "off"] },
      { id: "color", label: "Color output", currentValue: "on", values: ["on", "off"] },
    ];

    await ctx.ui.custom((_tui, theme, _kb, done) => {
      const container = new Container();
      container.addChild(new Text(theme.fg("accent", theme.bold("Settings")), 1, 1));

      const settingsList = new SettingsList(
        items,
        Math.min(items.length + 2, 15),
        getSettingsListTheme(),
        (id, newValue) => {
          // Handle value change
          ctx.ui.notify(`${id} = ${newValue}`, "info");
        },
        () => done(undefined),  // On close
        { enableSearch: true }, // Optional: enable fuzzy search by label
      );
      container.addChild(settingsList);

      return {
        get focused() { return settingsList.focused; },
        set focused(value) { settingsList.focused = value; },
        render: (w) => container.render(w),
        invalidate: () => container.invalidate(),
        handleInput: (data) => settingsList.handleInput?.(data),
      };
    });
  },
});
```

With `enableSearch`, typing fuzzy-filters labels, Backspace edits the query, and Up/Down plus Enter/Space navigate and activate matches. `SettingsList` propagates focus to its search input and nested focusable submenus for IME cursor placement; a wrapper returned from `ctx.ui.custom()` must forward `focused` as shown above.

**Examples:** [tools.ts](../examples/extensions/tools.ts)

### Pattern 4: Persistent Status Indicator

Show status in the footer that persists across renders. Good for mode indicators.

```typescript
// Set status (shown in footer)
ctx.ui.setStatus("my-ext", ctx.ui.theme.fg("accent", "● active"));

// Clear status
ctx.ui.setStatus("my-ext", undefined);
```

**Examples:** [status-line.ts](../examples/extensions/status-line.ts), [plan-mode.ts](../examples/extensions/plan-mode.ts), [preset.ts](../examples/extensions/preset.ts)

### Pattern 4b: Working Indicator Customization

Customize the inline working indicator shown while Scramjet is streaming a response.

```typescript
// Static indicator
ctx.ui.setWorkingIndicator({ frames: [ctx.ui.theme.fg("accent", "●")] });

// Custom animated indicator
ctx.ui.setWorkingIndicator({
  frames: [
    ctx.ui.theme.fg("dim", "·"),
    ctx.ui.theme.fg("muted", "•"),
    ctx.ui.theme.fg("accent", "●"),
    ctx.ui.theme.fg("muted", "•"),
  ],
  intervalMs: 120,
});

// Hide the indicator entirely
ctx.ui.setWorkingIndicator({ frames: [] });

// Restore Scramjet's default spinner
ctx.ui.setWorkingIndicator();
```

This only affects the normal streaming working indicator. Compaction and retry loaders keep their built-in styling. Custom frames are rendered verbatim, so extensions must add their own colors when needed.

**Examples:** [working-indicator.ts](../examples/extensions/working-indicator.ts)

### Pattern 5: Widgets Above/Below Editor

Show persistent content above or below the input editor. Good for todo lists, progress.

```typescript
// Simple string array (above editor by default)
ctx.ui.setWidget("my-widget", ["Line 1", "Line 2"]);

// Render below the editor
ctx.ui.setWidget("my-widget", ["Line 1", "Line 2"], { placement: "belowEditor" });

// Or with theme
ctx.ui.setWidget("my-widget", (_tui, theme) => {
  const lines = items.map((item, i) =>
    item.done
      ? theme.fg("success", "✓ ") + theme.fg("muted", item.text)
      : theme.fg("dim", "○ ") + item.text
  );
  return {
    render: () => lines,
    invalidate: () => {},
  };
});

// Clear
ctx.ui.setWidget("my-widget", undefined);
```

**Examples:** [plan-mode.ts](../examples/extensions/plan-mode.ts)

### Pattern 6: Custom Footer

Replace the footer. `footerData` exposes data not otherwise accessible to extensions.

```typescript
ctx.ui.setFooter((tui, theme, footerData) => ({
  invalidate() {},
  render(width: number): string[] {
    // footerData.getGitBranch(): string | null
    // footerData.getExtensionStatuses(): ReadonlyMap<string, string>
    return [`${ctx.model?.id} (${footerData.getGitBranch() || "no git"})`];
  },
  dispose: footerData.onBranchChange(() => tui.requestRender()), // reactive
}));

ctx.ui.setFooter(undefined); // restore default
```

Token stats available via `ctx.sessionManager.getBranch()` and `ctx.model`.

**Examples:** [custom-footer.ts](../examples/extensions/custom-footer.ts)

### Pattern 7: Custom Editor (vim mode, etc.)

Replace the main input editor with a custom implementation. Useful for modal editing (vim), different keybindings (emacs), or specialized input handling.

```typescript
import { CustomEditor, type ExtensionAPI } from "@leanandmean/coding-agent";
import { decodeKittyPrintable, matchesKey, truncateToWidth } from "@leanandmean/tui";

type Mode = "normal" | "insert";

class VimEditor extends CustomEditor {
  private mode: Mode = "insert";

  handleInput(data: string): void {
    // Escape: switch to normal mode, or pass through for app handling
    if (matchesKey(data, "escape")) {
      if (this.mode === "insert") {
        this.mode = "normal";
        return;
      }
      // In normal mode, escape aborts agent (handled by CustomEditor)
      super.handleInput(data);
      return;
    }

    // Insert mode: pass everything to CustomEditor
    if (this.mode === "insert") {
      super.handleInput(data);
      return;
    }

    if (matchesKey(data, "enter")) {
      super.handleInput(data);
      return;
    }

    // Normal mode: vim-style navigation
    const key = decodeKittyPrintable(data) ?? data;
    switch (key) {
      case "i": this.mode = "insert"; return;
      case "h": super.handleInput("\x1b[D"); return; // Left
      case "j": super.handleInput("\x1b[B"); return; // Down
      case "k": super.handleInput("\x1b[A"); return; // Up
      case "l": super.handleInput("\x1b[C"); return; // Right
    }
    // Pass unhandled keys to super (ctrl+c, etc.), but filter printable chars
    if (key.length === 1 && key.charCodeAt(0) >= 32) return;
    super.handleInput(data);
  }

  render(width: number): string[] {
    const lines = super.render(width);
    // Add mode indicator to bottom border (use truncateToWidth for ANSI-safe truncation)
    if (lines.length >= 3) {
      const label = this.mode === "normal" ? " NORMAL " : " INSERT ";
      const lastLine = lines[lines.length - 1]!;
      // Pass "" as ellipsis to avoid adding "..." when truncating
      lines[lines.length - 1] = truncateToWidth(lastLine, width - label.length, "") + label;
    }
    return lines;
  }
}

export default function (pi: ExtensionAPI) {
  pi.on("session_start", (_event, ctx) => {
    // Factory receives theme and keybindings from the app
    ctx.ui.setEditorComponent((tui, theme, keybindings) =>
      new VimEditor(theme, keybindings)
    );
  });
}
```

**Key points:**

- **Extend `CustomEditor`** (not base `Editor`) to get app keybindings (escape to abort, ctrl+d to exit, model switching, etc.)
- **Call `super.handleInput(data)`** for keys you don't handle
- **Factory pattern**: `setEditorComponent` receives a factory function that gets `tui`, `theme`, and `keybindings`
- **Pass `undefined`** to restore the default editor: `ctx.ui.setEditorComponent(undefined)`

**Examples:** [modal-editor.ts](../examples/extensions/modal-editor.ts)

## Key Rules

1. **Always use theme from callback** - Don't import theme directly. Use `theme` from the `ctx.ui.custom((tui, theme, keybindings, done) => ...)` callback.

2. **Always type DynamicBorder color param** - Write `(s: string) => theme.fg("accent", s)`, not `(s) => theme.fg("accent", s)`.

3. **Call tui.requestRender() after state changes** - In `handleInput`, call `tui.requestRender()` after updating state.

4. **Return the three-method object** - Custom components need `{ render, invalidate, handleInput }`.

5. **Use existing components** - `SelectList`, `SettingsList`, `BorderedLoader` cover 90% of cases. Don't rebuild them.

## Examples

- **Selection UI**: [examples/extensions/preset.ts](../examples/extensions/preset.ts) - SelectList with DynamicBorder framing
- **Async with cancel**: [examples/extensions/qna.ts](../examples/extensions/qna.ts) - BorderedLoader for LLM calls
- **Settings toggles**: [examples/extensions/tools.ts](../examples/extensions/tools.ts) - SettingsList for tool enable/disable
- **Status indicators**: [examples/extensions/plan-mode.ts](../examples/extensions/plan-mode.ts) - setStatus and setWidget
- **Working indicator**: [examples/extensions/working-indicator.ts](../examples/extensions/working-indicator.ts) - setWorkingIndicator
- **Custom footer**: [examples/extensions/custom-footer.ts](../examples/extensions/custom-footer.ts) - setFooter with stats
- **Custom editor**: [examples/extensions/modal-editor.ts](../examples/extensions/modal-editor.ts) - Vim-like modal editing
- **Snake game**: [examples/extensions/snake.ts](../examples/extensions/snake.ts) - Full game with keyboard input, game loop
- **Custom tool rendering**: [examples/extensions/todo.ts](../examples/extensions/todo.ts) - renderCall and renderResult
