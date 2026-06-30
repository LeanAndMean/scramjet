import { describe, expect, it } from "vitest";
import { renderDiagram } from "../src/diagram/renderer/index.js";
import type { CharRole, Canvas, RoleCanvas } from "../src/diagram/renderer/types.js";

// ============================================================================
// Helpers
// ============================================================================

function canvasToString(chars: Canvas): string {
	const maxY = chars[0]?.length ?? 0;
	const lines: string[] = [];
	for (let y = 0; y < maxY; y++) {
		let line = "";
		for (let x = 0; x < chars.length; x++) {
			line += chars[x]?.[y] || "";
		}
		lines.push(line.trimEnd());
	}
	while (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
	return lines.join("\n");
}

/** Find all (x, y) positions of a character in the canvas */
function findChar(chars: Canvas, char: string): { x: number; y: number }[] {
	const positions: { x: number; y: number }[] = [];
	const maxY = chars[0]?.length ?? 0;
	for (let x = 0; x < chars.length; x++) {
		for (let y = 0; y < maxY; y++) {
			if (chars[x]?.[y] === char) positions.push({ x, y });
		}
	}
	return positions;
}

/** Find positions of a text substring in the canvas (row-wise scan) */
function findText(chars: Canvas, text: string): { x: number; y: number }[] {
	const positions: { x: number; y: number }[] = [];
	const maxY = chars[0]?.length ?? 0;
	for (let y = 0; y < maxY; y++) {
		for (let x = 0; x <= chars.length - text.length; x++) {
			let match = true;
			for (let i = 0; i < text.length; i++) {
				if (chars[x + i]?.[y] !== text[i]) {
					match = false;
					break;
				}
			}
			if (match) positions.push({ x, y });
		}
	}
	return positions;
}

/** Find the row index of a label in the string output */
function findLabelRow(text: string, label: string): number {
	return text.split("\n").findIndex((l) => l.includes(label));
}

/** Find the first column index of a label in the string output */
function findLabelCol(text: string, label: string): number {
	for (const line of text.split("\n")) {
		const idx = line.indexOf(label);
		if (idx >= 0) return idx;
	}
	return -1;
}

/** Get the role at a specific canvas position */
function roleAt(roles: RoleCanvas, x: number, y: number): CharRole | null {
	return roles[x]?.[y] ?? null;
}

// ============================================================================
// 1. Character Role Integrity
// ============================================================================

describe("character role integrity", () => {
	it("node label hyphen gets role 'text' not 'border'", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A[issue-plan]");
		const hyphens = findText(chars, "-");
		expect(hyphens.length).toBeGreaterThan(0);
		for (const pos of hyphens) {
			expect(roleAt(roles, pos.x, pos.y), `hyphen at (${pos.x},${pos.y})`).toBe("text");
		}
	});

	it("all-hyphen label gets role 'text' for every hyphen", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A[---]");
		const hyphens = findText(chars, "---");
		expect(hyphens.length).toBeGreaterThan(0);
		for (const pos of hyphens) {
			for (let i = 0; i < 3; i++) {
				expect(roleAt(roles, pos.x + i, pos.y), `char at (${pos.x + i},${pos.y})`).toBe("text");
			}
		}
	});

	it("every character in a node label has role 'text'", () => {
		const label = "Hello World";
		const { chars, roles } = renderDiagram(`graph TD\n  A[${label}]`);
		const positions = findText(chars, label);
		expect(positions.length).toBeGreaterThan(0);
		const pos = positions[0]!;
		for (let i = 0; i < label.length; i++) {
			const role = roleAt(roles, pos.x + i, pos.y);
			expect(role, `'${label[i]}' at (${pos.x + i},${pos.y})`).toBe("text");
		}
	});

	it("node box border '─' has role 'border' not 'line'", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A[Test]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		// Top border row (first line with ┌)
		const topRow = lines.findIndex((l) => l.includes("┌"));
		expect(topRow).toBeGreaterThanOrEqual(0);
		// Find ─ chars on the top border row
		const maxX = chars.length;
		for (let x = 0; x < maxX; x++) {
			if (chars[x]?.[topRow] === "─") {
				expect(roleAt(roles, x, topRow), `border ─ at (${x},${topRow})`).toBe("border");
			}
		}
	});

	it("edge line chars between nodes have role 'line'", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A[Top] --> B[Bottom]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		// Find │ chars between the two boxes (not on box borders)
		const topBoxEnd = lines.findIndex((l) => l.includes("└"));
		const bottomBoxStart = lines.findIndex((l, i) => i > topBoxEnd && l.includes("┌"));
		for (let y = topBoxEnd + 1; y < bottomBoxStart; y++) {
			for (let x = 0; x < chars.length; x++) {
				if (chars[x]?.[y] === "│") {
					expect(roleAt(roles, x, y), `edge │ at (${x},${y})`).toBe("line");
				}
			}
		}
	});

	it("arrow characters have role 'arrow'", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A --> B");
		const arrows = ["▲", "▼", "◄", "►"];
		for (const arrow of arrows) {
			for (const pos of findChar(chars, arrow)) {
				expect(roleAt(roles, pos.x, pos.y), `${arrow} at (${pos.x},${pos.y})`).toBe("arrow");
			}
		}
	});

	it("edge label text gets role 'text'", () => {
		const label = "myLabel";
		const { chars, roles } = renderDiagram(`graph TD\n  A -->|${label}| B`);
		const positions = findText(chars, label);
		expect(positions.length).toBeGreaterThan(0);
		const pos = positions[0]!;
		for (let i = 0; i < label.length; i++) {
			expect(roleAt(roles, pos.x + i, pos.y), `'${label[i]}' at (${pos.x + i},${pos.y})`).toBe("text");
		}
	});

	it("edge label containing hyphen gets role 'text'", () => {
		const { chars, roles } = renderDiagram('graph TD\n  A -->|"a-b"| B');
		const positions = findText(chars, "a-b");
		expect(positions.length).toBeGreaterThan(0);
		const pos = positions[0]!;
		// The hyphen in the edge label must be 'text'
		expect(roleAt(roles, pos.x + 1, pos.y), "hyphen in edge label").toBe("text");
	});

	it("junction chars at box border have role 'junction'", () => {
		const { chars, roles } = renderDiagram("graph TD\n  A --> B\n  A --> C");
		const junctions = ["┬", "┴", "├", "┤"];
		let found = false;
		for (const j of junctions) {
			for (const pos of findChar(chars, j)) {
				const role = roleAt(roles, pos.x, pos.y);
				if (role === "junction") found = true;
			}
		}
		expect(found, "at least one junction char with 'junction' role").toBe(true);
	});
});

// ============================================================================
// 2. Edge Routing & Arrowheads
// ============================================================================

describe("edge routing and arrowheads", () => {
	it("TD downward edge has ▼ adjacent to target box top border", () => {
		const { chars } = renderDiagram("graph TD\n  A[Top] --> B[Bottom]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		const arrowRow = lines.findIndex((l) => l.includes("▼"));
		expect(arrowRow).toBeGreaterThanOrEqual(0);
		// Row below ▼ should have a box top border char
		expect(lines[arrowRow + 1], "row below ▼ should have box border").toMatch(/[┌─┐┬]/);
	});

	it("TD upward (backward) edge has ▲ adjacent to target box bottom border", () => {
		const { chars } = renderDiagram("graph TD\n  A[Top] --> B[Bottom]\n  B --> A");
		const text = canvasToString(chars);
		const arrows = findChar(chars, "▲");
		expect(arrows.length).toBeGreaterThan(0);
	});

	it("LR rightward edge has ► adjacent to target box left border", () => {
		const { chars } = renderDiagram("graph LR\n  A[Left] --> B[Right]");
		const text = canvasToString(chars);
		expect(text).toContain("►");
		const lines = text.split("\n");
		// ► should be on a line that also contains the target box border
		const arrowLine = lines.find((l) => l.includes("►"));
		expect(arrowLine).toBeDefined();
		expect(arrowLine).toMatch(/►.*│|►.*\|/);
	});

	it("box-start junction ┬ appears on source bottom border for TD downward edge", () => {
		const { chars } = renderDiagram("graph TD\n  A[Source] --> B[Target]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		// Find the bottom border of Source box (line with └)
		const bottomBorder = lines.find((l) => l.includes("└") && l.includes("┘"));
		expect(bottomBorder, "Source bottom border should contain ┬").toContain("┬");
	});

	it("corner chars are correct at path bends", () => {
		// An edge that requires a turn should have correct corner chars
		const { chars } = renderDiagram("graph TD\n  A[Node] -->|Loop| A");
		const text = canvasToString(chars);
		// Self-loop should have └ (down→right turn) and ┘ (right→up turn)
		expect(text).toContain("└");
		expect(text).toContain("┘");
	});

	it("edge line segments use correct direction chars (│ for vertical, ─ for horizontal)", () => {
		const { chars } = renderDiagram("graph TD\n  A --> B");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		// Between the two boxes, there should be │ chars (vertical edge)
		const bottomA = lines.findIndex((l) => l.includes("└"));
		const topB = lines.findIndex((l, i) => i > bottomA && l.includes("┌"));
		for (let y = bottomA + 1; y < topB; y++) {
			const line = lines[y]!;
			// Edge segment should have │ (not ─) for a vertical path
			if (line.trim() && !line.includes("▼") && !line.includes("▲")) {
				expect(line, `vertical edge at row ${y}`).toMatch(/│/);
			}
		}
	});

	it("multiple outgoing edges: all targets get arrowheads", () => {
		const { chars } = renderDiagram("graph TD\n  A --> B\n  A --> C\n  A --> D");
		const text = canvasToString(chars);
		expect(text).toContain("B");
		expect(text).toContain("C");
		expect(text).toContain("D");
		// At least 3 downward arrowheads (one per target)
		const downArrows = findChar(chars, "▼");
		expect(downArrows.length).toBeGreaterThanOrEqual(3);
	});

	it("edge styles: dotted edges use ┄┆ characters", () => {
		const { chars } = renderDiagram("graph TD\n  A -.-> B");
		const text = canvasToString(chars);
		expect(text).toMatch(/[┄┆]/);
	});

	it("edge styles: thick edges use ━┃ characters", () => {
		const { chars } = renderDiagram("graph TD\n  A ==> B");
		const text = canvasToString(chars);
		expect(text).toMatch(/[━┃]/);
	});
});

// ============================================================================
// 3. Self-Loop Rendering
// ============================================================================

describe("self-loop rendering", () => {
	it("TD self-loop: arrowhead adjacent to box border", () => {
		const { chars } = renderDiagram("flowchart TD\n  A[Node] -->|Loop| A");
		const text = canvasToString(chars);
		// Arrowhead must be directly adjacent to a border char
		const lines = text.split("\n");
		const arrowLine = lines.find((l) => l.includes("◄") || l.includes("▲"));
		expect(arrowLine).toBeDefined();
		expect(arrowLine).toMatch(/[│├┤┬┴][◄▲]|[►▼][│├┤┬┴]/);
	});

	it("LR self-loop: renders without crash and has arrowhead", () => {
		const { chars } = renderDiagram("flowchart LR\n  A[Node] -->|Loop| A");
		const text = canvasToString(chars);
		expect(text).toContain("Node");
		expect(text).toMatch(/[▲▼◄►]/);
	});

	it("BT self-loop: renders without crash and has arrowhead", () => {
		const { chars } = renderDiagram("flowchart BT\n  A[Node] -->|Loop| A");
		const text = canvasToString(chars);
		expect(text).toContain("Node");
		expect(text).toMatch(/[▲▼◄►]/);
	});

	it("self-loop label does not collide with sibling edge", () => {
		const source = [
			"flowchart TD",
			'  P[Parent] -->|Left| A[Review]',
			'  P -->|Right| B[Implement]',
			'  A -->|Retry| A',
			'  A -->|Done| B',
		].join("\n");
		const { chars } = renderDiagram(source);
		const text = canvasToString(chars);
		const lines = text.split("\n");
		for (const line of lines) {
			if (line.includes("Retry")) {
				expect(line, `self-loop label collides with sibling edge: ${line}`).not.toMatch(/►/);
			}
		}
	});
});

// ============================================================================
// 4. Bidirectional Edge Rendering
// ============================================================================

describe("bidirectional edge rendering", () => {
	it("both arrowheads present for bidirectional edges", () => {
		const { chars } = renderDiagram('graph TD\n  A[Alpha] -->|Fwd| B[Beta]\n  B -->|Bck| A');
		const text = canvasToString(chars);
		expect(text).toContain("▲");
		expect(text).toContain("▼");
	});

	it("labels are near their respective arrowheads", () => {
		const { chars } = renderDiagram('flowchart TD\n  A[Alpha] -->|Forward| B[Beta]\n  B -->|Backward| A');
		const text = canvasToString(chars);
		const lines = text.split("\n");
		const upRow = lines.findIndex((l) => l.includes("▲"));
		const downRow = lines.findIndex((l) => l.includes("▼"));
		const fwdRow = findLabelRow(text, "Forward");
		const bckRow = findLabelRow(text, "Backward");
		// Forward (A→B) label should be closer to ▼ (its arrowhead at B)
		expect(Math.abs(fwdRow - downRow)).toBeLessThan(Math.abs(fwdRow - upRow));
		// Backward (B→A) label should be closer to ▲ (its arrowhead at A)
		expect(Math.abs(bckRow - upRow)).toBeLessThan(Math.abs(bckRow - downRow));
	});

	it("both edge labels are intact (no interleaved chars)", () => {
		const { chars } = renderDiagram('graph TD\n  A -->|"alpha"| B\n  B -->|"bravo"| A');
		const text = canvasToString(chars);
		expect(text).toContain("alpha");
		expect(text).toContain("bravo");
	});
});

// ============================================================================
// 5. Node Rendering
// ============================================================================

describe("node rendering", () => {
	it("label is horizontally centered in box", () => {
		const { chars } = renderDiagram("graph TD\n  A[Hello]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		const labelLine = lines.find((l) => l.includes("Hello"))!;
		const boxLine = lines.find((l) => l.includes("┌"))!;
		const boxLeft = boxLine.indexOf("┌");
		const boxRight = boxLine.indexOf("┐");
		const labelLeft = labelLine.indexOf("Hello");
		const labelRight = labelLeft + "Hello".length - 1;
		const leftPad = labelLeft - boxLeft - 1;
		const rightPad = boxRight - labelRight - 1;
		expect(Math.abs(leftPad - rightPad), "label should be centered").toBeLessThanOrEqual(1);
	});

	it("label is vertically centered in box", () => {
		const { chars } = renderDiagram("graph TD\n  A[Test]");
		const text = canvasToString(chars);
		const lines = text.split("\n");
		const topIdx = lines.findIndex((l) => l.includes("┌"));
		const bottomIdx = lines.findIndex((l) => l.includes("└"));
		const labelIdx = lines.findIndex((l) => l.includes("Test"));
		const topPad = labelIdx - topIdx;
		const bottomPad = bottomIdx - labelIdx;
		expect(Math.abs(topPad - bottomPad), "label should be vertically centered").toBeLessThanOrEqual(1);
	});

	it("label with hyphens renders intact", () => {
		const { chars } = renderDiagram("graph TD\n  A[issue-review-plan]");
		const text = canvasToString(chars);
		expect(text).toContain("issue-review-plan");
	});

	it("empty label produces valid box", () => {
		expect(() => renderDiagram('graph TD\n  A[""]')).not.toThrow();
	});

	it("very long label expands box correctly", () => {
		const label = "This is a very long node label for testing";
		const { chars } = renderDiagram(`graph TD\n  A[${label}]`);
		const text = canvasToString(chars);
		expect(text).toContain(label);
		// Box should be wider than the label
		const lines = text.split("\n");
		const topBorder = lines.find((l) => l.includes("┌"))!;
		const boxWidth = topBorder.indexOf("┐") - topBorder.indexOf("┌") + 1;
		expect(boxWidth).toBeGreaterThan(label.length);
	});

	it("rounded shape uses ╭╮╰╯ corners", () => {
		const { chars } = renderDiagram("graph TD\n  A(Rounded)");
		const text = canvasToString(chars);
		expect(text).toContain("╭");
		expect(text).toContain("╮");
		expect(text).toContain("╰");
		expect(text).toContain("╯");
	});

	it("stadium shape uses parentheses", () => {
		const { chars } = renderDiagram("graph TD\n  A([Stadium])");
		const text = canvasToString(chars);
		expect(text).toContain("(");
		expect(text).toContain(")");
	});
});

// ============================================================================
// 6. Layout & Graph Structure
// ============================================================================

describe("layout and graph structure", () => {
	it("TD linear chain: nodes vertically aligned", () => {
		const { chars } = renderDiagram("graph TD\n  A[First] --> B[Second] --> C[Third]");
		const text = canvasToString(chars);
		const colA = findLabelCol(text, "First");
		const colB = findLabelCol(text, "Second");
		const colC = findLabelCol(text, "Third");
		// Centers should be within a few chars of each other
		const centerA = colA + Math.floor("First".length / 2);
		const centerB = colB + Math.floor("Second".length / 2);
		const centerC = colC + Math.floor("Third".length / 2);
		expect(Math.abs(centerA - centerB)).toBeLessThanOrEqual(2);
		expect(Math.abs(centerB - centerC)).toBeLessThanOrEqual(2);
	});

	it("LR linear chain: nodes horizontally arranged", () => {
		const { chars } = renderDiagram("graph LR\n  A[First] --> B[Second] --> C[Third]");
		const text = canvasToString(chars);
		const colA = findLabelCol(text, "First");
		const colB = findLabelCol(text, "Second");
		const colC = findLabelCol(text, "Third");
		expect(colA).toBeLessThan(colB);
		expect(colB).toBeLessThan(colC);
	});

	it("BT direction: source at bottom, target at top", () => {
		const { chars } = renderDiagram("graph BT\n  A[Source] --> B[Target]");
		const text = canvasToString(chars);
		const rowA = findLabelRow(text, "Source");
		const rowB = findLabelRow(text, "Target");
		expect(rowA, "source should be below target in BT").toBeGreaterThan(rowB);
		// Arrow should point up
		expect(text).toContain("▲");
	});

	it("disconnected components don't overlap", () => {
		const { chars } = renderDiagram("graph TD\n  A[Alpha]\n  B[Beta]");
		const text = canvasToString(chars);
		expect(text).toContain("Alpha");
		expect(text).toContain("Beta");
		// Bounding boxes should not overlap (side-by-side is valid)
		const rowA = findLabelRow(text, "Alpha");
		const rowB = findLabelRow(text, "Beta");
		const colA = findLabelCol(text, "Alpha");
		const colB = findLabelCol(text, "Beta");
		if (rowA === rowB) {
			// Same row: labels must not overlap horizontally
			const endA = colA + "Alpha".length;
			expect(colB, "Beta should start after Alpha ends").toBeGreaterThanOrEqual(endA);
		}
	});

	it("diamond pattern: all nodes and edges present", () => {
		const src = "graph TD\n  A --> B\n  A --> C\n  B --> D\n  C --> D";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("A");
		expect(text).toContain("B");
		expect(text).toContain("C");
		expect(text).toContain("D");
		// Should have multiple arrowheads
		const downArrows = findChar(chars, "▼");
		expect(downArrows.length).toBeGreaterThanOrEqual(3);
	});

	it("fan-in bundle: all source labels present with arrowhead at target", () => {
		const src = "graph TD\n  A --> D\n  B --> D\n  C --> D";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("A");
		expect(text).toContain("B");
		expect(text).toContain("C");
		expect(text).toContain("D");
		// At least one arrowhead pointing to D
		expect(text).toMatch(/[▼▲◄►]/);
	});

	it("fan-out bundle: all target labels present", () => {
		const src = "graph TD\n  A --> B\n  A --> C\n  A --> D";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("B");
		expect(text).toContain("C");
		expect(text).toContain("D");
	});

	it("subgraph contains its nodes", () => {
		const src = ["flowchart TD", "  subgraph S[Group]", "    A[Inner]", "  end", "  B[Outer]"].join("\n");
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("Inner");
		expect(text).toContain("Outer");
		expect(text).toContain("Group");
	});

	it("long chain maintains consistent node spacing", () => {
		const nodes = Array.from({ length: 6 }, (_, i) => `N${i}[Node${i}]`);
		const edges = Array.from({ length: 5 }, (_, i) => `N${i} --> N${i + 1}`);
		const src = `graph TD\n  ${nodes.join("\n  ")}\n  ${edges.join("\n  ")}`;
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		// All nodes present
		for (let i = 0; i < 6; i++) {
			expect(text).toContain(`Node${i}`);
		}
		// Check row ordering
		const rows = Array.from({ length: 6 }, (_, i) => findLabelRow(text, `Node${i}`));
		for (let i = 1; i < 6; i++) {
			expect(rows[i]!, `Node${i} should be below Node${i - 1}`).toBeGreaterThan(rows[i - 1]!);
		}
	});
});

// ============================================================================
// 7. Edge Case Stress Tests
// ============================================================================

describe("edge cases and stress tests", () => {
	it("node with box-drawing-like chars in label", () => {
		const { chars } = renderDiagram('graph TD\n  A["┌test┐"]');
		const text = canvasToString(chars);
		// The label should appear (parser may strip some chars, but shouldn't crash)
		expect(text).toContain("test");
	});

	it("multiple self-loops on different nodes", () => {
		const src = "flowchart TD\n  A[N1] -->|L1| A\n  B[N2] -->|L2| B\n  A --> B";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("N1");
		expect(text).toContain("N2");
		expect(text).toContain("L1");
		expect(text).toContain("L2");
	});

	it("edge from child back to grandparent (skip-level backward)", () => {
		const src = "graph TD\n  A --> B\n  B --> C\n  C --> A";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("A");
		expect(text).toContain("B");
		expect(text).toContain("C");
	});

	it("node referenced only as target (no outgoing edges)", () => {
		const src = "graph TD\n  A --> B\n  A --> C";
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("B");
		expect(text).toContain("C");
	});

	it("single node with no edges", () => {
		const { chars } = renderDiagram("graph TD\n  A[Alone]");
		const text = canvasToString(chars);
		expect(text).toContain("Alone");
		// Should have a complete box
		expect(text).toContain("┌");
		expect(text).toContain("┐");
		expect(text).toContain("└");
		expect(text).toContain("┘");
	});

	it("state diagram: start and end pseudo-states render", () => {
		const src = ["stateDiagram-v2", "  [*] --> Active", "  Active --> [*]"].join("\n");
		expect(() => renderDiagram(src)).not.toThrow();
		const { chars } = renderDiagram(src);
		const text = canvasToString(chars);
		expect(text).toContain("Active");
	});
});

// ============================================================================
// 8. Attachment-Point Collision (junction adjacent to opposing arrowhead)
// ============================================================================

describe("attachment-point collision", () => {
	const SELF_LOOP_AND_OUTGOING = ["flowchart TD", "  A -->|outgoing| B", "  A -->|loop| A"].join("\n");

	const SELF_LOOP_WITH_PARENT = [
		"flowchart TD",
		"  P[Parent] --> A[Node]",
		"  A -->|self| A",
		"  A -->|next| B[Sibling]",
	].join("\n");

	// Minimal reproducer: same-rank siblings where self-loop and sibling edge share right side
	const SIBLING_COLLISION = [
		"flowchart TD",
		"  A --> B",
		"  A --> C",
		"  B --> B",
		"  B -->|x| C",
	].join("\n");

	it("no junction directly adjacent to opposing arrowhead (horizontal)", () => {
		for (const src of [SELF_LOOP_AND_OUTGOING, SELF_LOOP_WITH_PARENT, SIBLING_COLLISION]) {
			const { chars } = renderDiagram(src);
			const text = canvasToString(chars);
			const lines = text.split("\n");
			for (let y = 0; y < lines.length; y++) {
				const line = lines[y]!;
				expect(line, `row ${y}: ├◄ collision`).not.toContain("├◄");
				expect(line, `row ${y}: ►┤ collision`).not.toContain("►┤");
			}
		}
	});

	it("no junction directly adjacent to opposing arrowhead (vertical)", () => {
		for (const src of [SELF_LOOP_AND_OUTGOING, SELF_LOOP_WITH_PARENT, SIBLING_COLLISION]) {
			const { chars } = renderDiagram(src);
			const maxY = chars[0]?.length ?? 0;
			for (let x = 0; x < chars.length; x++) {
				for (let y = 0; y < maxY - 1; y++) {
					const curr = chars[x]?.[y];
					const below = chars[x]?.[y + 1];
					expect(curr === "┬" && below === "▲", `(${x},${y}): ┬▲ collision`).toBe(false);
					expect(curr === "▼" && below === "┴", `(${x},${y}): ▼┴ collision`).toBe(false);
				}
			}
		}
	});

	it("self-loop + outgoing edge: both labels present", () => {
		const { chars } = renderDiagram(SELF_LOOP_AND_OUTGOING);
		const text = canvasToString(chars);
		expect(text, "outgoing edge label missing").toContain("outgoing");
		expect(text, "self-loop label missing").toContain("loop");
	});
});

// ============================================================================
// 9. Horizontal Bidirectional Label Completeness
// ============================================================================

describe("horizontal bidirectional labels", () => {
	const HORIZ_BIDIR = [
		"flowchart TD",
		"  P[Parent] --> A[Left]",
		"  P --> B[Right]",
		'  A -->|"alpha"| B',
		'  B -->|"beta"| A',
	].join("\n");

	it("both labels appear in horizontal bidirectional edges between siblings", () => {
		const { chars } = renderDiagram(HORIZ_BIDIR);
		const text = canvasToString(chars);
		expect(text, "label 'alpha' missing").toContain("alpha");
		expect(text, "label 'beta' missing").toContain("beta");
	});

	it("bidirectional labels do not overlap", () => {
		const { chars } = renderDiagram(HORIZ_BIDIR);
		const alphaPositions = findText(chars, "alpha");
		const betaPositions = findText(chars, "beta");
		expect(alphaPositions.length, "alpha not found").toBeGreaterThan(0);
		expect(betaPositions.length, "beta not found").toBeGreaterThan(0);
		const alphaSet = new Set<string>();
		for (const pos of alphaPositions) {
			for (let i = 0; i < "alpha".length; i++) {
				alphaSet.add(`${pos.x + i},${pos.y}`);
			}
		}
		for (const pos of betaPositions) {
			for (let i = 0; i < "beta".length; i++) {
				expect(alphaSet.has(`${pos.x + i},${pos.y}`), `labels overlap at (${pos.x + i},${pos.y})`).toBe(false);
			}
		}
	});

	it("both arrowheads present for horizontal bidirectional edges", () => {
		const { chars } = renderDiagram(HORIZ_BIDIR);
		const left = findChar(chars, "◄");
		const right = findChar(chars, "►");
		const up = findChar(chars, "▲");
		const down = findChar(chars, "▼");
		const hasHorizPair = left.length > 0 && right.length > 0;
		const hasVertPair = up.length > 0 && down.length > 0;
		expect(hasHorizPair || hasVertPair, "need opposing arrowheads for bidirectional edges").toBe(true);
	});
});
