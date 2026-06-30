import { describe, expect, it } from "vitest";
import { parseMermaid } from "../src/diagram/parser.js";

describe("parseMermaid", () => {
	describe("header parsing", () => {
		it("accepts graph TD", () => {
			const g = parseMermaid("graph TD\n  A --> B");
			expect(g.direction).toBe("TD");
		});

		it("accepts flowchart LR", () => {
			const g = parseMermaid("flowchart LR\n  A --> B");
			expect(g.direction).toBe("LR");
		});

		it("accepts stateDiagram-v2", () => {
			const g = parseMermaid("stateDiagram-v2\n  s1 --> s2");
			expect(g.direction).toBe("TD");
		});

		it("parses all direction variants", () => {
			for (const dir of ["TD", "TB", "LR", "RL", "BT"] as const) {
				const g = parseMermaid(`graph ${dir}\n  A --> B`);
				expect(g.direction).toBe(dir);
			}
		});
	});

	describe("node shapes", () => {
		it("parses rectangle shape A[text]", () => {
			const g = parseMermaid("graph TD\n  A[Hello]");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "rectangle" });
		});

		it("parses rounded shape A(text)", () => {
			const g = parseMermaid("graph TD\n  A(Hello)");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "rounded" });
		});

		it("parses diamond shape A{text}", () => {
			const g = parseMermaid("graph TD\n  A{Hello}");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "diamond" });
		});

		it("parses stadium shape A([text])", () => {
			const g = parseMermaid("graph TD\n  A([Hello])");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "stadium" });
		});

		it("parses circle shape A((text))", () => {
			const g = parseMermaid("graph TD\n  A((Hello))");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "circle" });
		});

		it("parses hexagon shape A{{text}}", () => {
			const g = parseMermaid("graph TD\n  A{{Hello}}");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "hexagon" });
		});

		it("parses subroutine shape A[[text]]", () => {
			const g = parseMermaid("graph TD\n  A[[Hello]]");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "subroutine" });
		});

		it("parses cylinder shape A[(text)]", () => {
			const g = parseMermaid("graph TD\n  A[(Hello)]");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "Hello", shape: "cylinder" });
		});

		it("creates implicit node with id-as-label for bare references", () => {
			const g = parseMermaid("graph TD\n  A --> B");
			expect(g.nodes.get("A")).toEqual({ id: "A", label: "A", shape: "rectangle" });
			expect(g.nodes.get("B")).toEqual({ id: "B", label: "B", shape: "rectangle" });
		});
	});

	describe("edge styles", () => {
		it("parses solid arrow -->", () => {
			const g = parseMermaid("graph TD\n  A --> B");
			expect(g.edges).toHaveLength(1);
			expect(g.edges[0]).toMatchObject({ source: "A", target: "B", style: "solid", hasArrowEnd: true });
		});

		it("parses dotted arrow -.->", () => {
			const g = parseMermaid("graph TD\n  A -.-> B");
			expect(g.edges).toHaveLength(1);
			expect(g.edges[0]).toMatchObject({ source: "A", target: "B", style: "dotted", hasArrowEnd: true });
		});

		it("parses thick arrow ==>", () => {
			const g = parseMermaid("graph TD\n  A ==> B");
			expect(g.edges).toHaveLength(1);
			expect(g.edges[0]).toMatchObject({ source: "A", target: "B", style: "thick", hasArrowEnd: true });
		});

		it("parses solid line without arrow ---", () => {
			const g = parseMermaid("graph TD\n  A --- B");
			expect(g.edges[0]).toMatchObject({ style: "solid", hasArrowEnd: false });
		});
	});

	describe("edge labels", () => {
		it("extracts pipe-delimited label -->|text|", () => {
			const g = parseMermaid("graph TD\n  A -->|yes| B");
			expect(g.edges[0]?.label).toBe("yes");
		});

		it("extracts text-embedded label -- text -->", () => {
			const g = parseMermaid("graph TD\n  A -- yes --> B");
			expect(g.edges[0]?.label).toBe("yes");
		});

		it("normalizes br tags in labels", () => {
			const g = parseMermaid("graph TD\n  A -->|line1<br>line2| B");
			expect(g.edges[0]?.label).toBe("line1\nline2");
		});
	});

	describe("bidirectional arrows", () => {
		it("parses <--> as bidirectional", () => {
			const g = parseMermaid("graph TD\n  A <--> B");
			expect(g.edges[0]).toMatchObject({ hasArrowStart: true, hasArrowEnd: true });
		});
	});

	describe("subgraphs", () => {
		it("parses simple subgraph with label", () => {
			const g = parseMermaid("graph TD\n  subgraph Backend\n    A --> B\n  end");
			expect(g.subgraphs).toHaveLength(1);
			expect(g.subgraphs[0]?.label).toBe("Backend");
			expect(g.subgraphs[0]?.nodeIds).toContain("A");
			expect(g.subgraphs[0]?.nodeIds).toContain("B");
		});

		it("parses subgraph id [label] form", () => {
			const g = parseMermaid("graph TD\n  subgraph sg1 [My Subgraph]\n    A\n  end");
			expect(g.subgraphs[0]?.id).toBe("sg1");
			expect(g.subgraphs[0]?.label).toBe("My Subgraph");
		});

		it("parses nested subgraphs", () => {
			const src = [
				"graph TD",
				"  subgraph outer [Outer]",
				"    subgraph inner [Inner]",
				"      A --> B",
				"    end",
				"  end",
			].join("\n");
			const g = parseMermaid(src);
			expect(g.subgraphs).toHaveLength(1);
			expect(g.subgraphs[0]?.children).toHaveLength(1);
			expect(g.subgraphs[0]?.children[0]?.label).toBe("Inner");
		});
	});

	describe("classDef and class assignment", () => {
		it("parses classDef with style properties", () => {
			const g = parseMermaid("graph TD\n  classDef highlight fill:#f9f,stroke:#333");
			expect(g.classDefs.get("highlight")).toEqual({ fill: "#f9f", stroke: "#333" });
		});

		it("parses class assignment", () => {
			const g = parseMermaid("graph TD\n  A[Node]\n  class A highlight");
			expect(g.classAssignments.get("A")).toBe("highlight");
		});

		it("parses ::: shorthand", () => {
			const g = parseMermaid("graph TD\n  A[Node]:::highlight --> B");
			expect(g.classAssignments.get("A")).toBe("highlight");
		});
	});

	describe("comments and separators", () => {
		it("ignores %% comments", () => {
			const g = parseMermaid("graph TD\n  %% this is a comment\n  A --> B");
			expect(g.nodes.size).toBe(2);
			expect(g.edges).toHaveLength(1);
		});

		it("ignores empty lines", () => {
			const g = parseMermaid("graph TD\n\n  A --> B\n\n");
			expect(g.nodes.size).toBe(2);
		});
	});

	describe("chain edges", () => {
		it("A --> B --> C produces 2 edges", () => {
			const g = parseMermaid("graph TD\n  A --> B --> C");
			expect(g.edges).toHaveLength(2);
			expect(g.edges[0]).toMatchObject({ source: "A", target: "B" });
			expect(g.edges[1]).toMatchObject({ source: "B", target: "C" });
		});

		it("A --> B --> C --> D produces 3 edges", () => {
			const g = parseMermaid("graph TD\n  A --> B --> C --> D");
			expect(g.edges).toHaveLength(3);
		});
	});

	describe("stateDiagram-v2", () => {
		it("parses states and transitions", () => {
			const g = parseMermaid("stateDiagram-v2\n  s1 --> s2");
			expect(g.nodes.has("s1")).toBe(true);
			expect(g.nodes.has("s2")).toBe(true);
			expect(g.edges).toHaveLength(1);
			expect(g.edges[0]).toMatchObject({ source: "s1", target: "s2" });
		});

		it("parses state descriptions", () => {
			const g = parseMermaid("stateDiagram-v2\n  s1 : Idle\n  s1 --> s2");
			expect(g.nodes.get("s1")?.label).toBe("Idle");
		});

		it("parses transition labels", () => {
			const g = parseMermaid("stateDiagram-v2\n  s1 --> s2 : trigger");
			expect(g.edges[0]?.label).toBe("trigger");
		});

		it("parses [*] start/end pseudostates", () => {
			const g = parseMermaid("stateDiagram-v2\n  [*] --> s1\n  s1 --> [*]");
			const startNode = [...g.nodes.values()].find((n) => n.shape === "state-start");
			const endNode = [...g.nodes.values()].find((n) => n.shape === "state-end");
			expect(startNode).toBeDefined();
			expect(endNode).toBeDefined();
		});

		it("parses composite states as subgraphs", () => {
			const src = ["stateDiagram-v2", "  state Processing {", "    inner1 --> inner2", "  }"].join("\n");
			const g = parseMermaid(src);
			expect(g.subgraphs).toHaveLength(1);
			expect(g.subgraphs[0]?.id).toBe("Processing");
			expect(g.subgraphs[0]?.nodeIds).toContain("inner1");
		});
	});

	describe("error cases", () => {
		it("throws on empty input", () => {
			expect(() => parseMermaid("")).toThrow(/empty/i);
		});

		it("throws on invalid header", () => {
			expect(() => parseMermaid("gantt\n  title Test")).toThrow(/invalid mermaid header/i);
		});

		it("throws on whitespace-only input", () => {
			expect(() => parseMermaid("   \n   ")).toThrow(/empty/i);
		});
	});
});
