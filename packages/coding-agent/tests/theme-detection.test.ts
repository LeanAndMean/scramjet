import { describe, expect, it } from "vitest";
import {
	classifyBackgroundColor,
	detectThemeFromEnvironment,
	relativeLuminance,
	srgbToLinear,
} from "../src/modes/interactive/theme/theme.js";

describe("srgbToLinear", () => {
	it("returns 0 for 0", () => {
		expect(srgbToLinear(0)).toBe(0);
	});

	it("returns 1 for 1", () => {
		expect(srgbToLinear(1)).toBe(1);
	});

	it("handles the linear segment (below 0.04045)", () => {
		expect(srgbToLinear(0.04045)).toBeCloseTo(0.04045 / 12.92, 10);
	});

	it("handles the gamma segment (above 0.04045)", () => {
		expect(srgbToLinear(0.5)).toBeCloseTo(((0.5 + 0.055) / 1.055) ** 2.4, 10);
	});
});

describe("relativeLuminance", () => {
	it("returns 0 for black", () => {
		expect(relativeLuminance(0, 0, 0)).toBe(0);
	});

	it("returns 1 for white", () => {
		expect(relativeLuminance(1, 1, 1)).toBeCloseTo(1, 5);
	});

	it("weighs green most heavily", () => {
		const rOnly = relativeLuminance(1, 0, 0);
		const gOnly = relativeLuminance(0, 1, 0);
		const bOnly = relativeLuminance(0, 0, 1);
		expect(gOnly).toBeGreaterThan(rOnly);
		expect(rOnly).toBeGreaterThan(bOnly);
	});
});

describe("classifyBackgroundColor", () => {
	it("classifies black (0,0,0) as dark", () => {
		expect(classifyBackgroundColor({ r: 0, g: 0, b: 0 })).toBe("dark");
	});

	it("classifies white (1,1,1) as light", () => {
		expect(classifyBackgroundColor({ r: 1, g: 1, b: 1 })).toBe("light");
	});

	it("classifies near-black as dark", () => {
		expect(classifyBackgroundColor({ r: 0.1, g: 0.1, b: 0.1 })).toBe("dark");
	});

	it("classifies near-white as light", () => {
		expect(classifyBackgroundColor({ r: 0.9, g: 0.9, b: 0.9 })).toBe("light");
	});

	it("classifies common dark terminal background (#1e1e1e) as dark", () => {
		expect(classifyBackgroundColor({ r: 0x1e / 255, g: 0x1e / 255, b: 0x1e / 255 })).toBe("dark");
	});

	it("classifies common light terminal background (#f5f5f5) as light", () => {
		expect(classifyBackgroundColor({ r: 0xf5 / 255, g: 0xf5 / 255, b: 0xf5 / 255 })).toBe("light");
	});

	it("has a luminance boundary at 0.2", () => {
		// Find an RGB gray value near the boundary
		// L = 0.2 corresponds to sRGB ~0.486 (inverse gamma)
		// For gray: relativeLuminance(x, x, x) = srgbToLinear(x)
		// srgbToLinear(0.486) ≈ 0.2
		const justBelow = { r: 0.48, g: 0.48, b: 0.48 };
		const justAbove = { r: 0.5, g: 0.5, b: 0.5 };
		expect(classifyBackgroundColor(justBelow)).toBe("dark");
		expect(classifyBackgroundColor(justAbove)).toBe("light");
	});
});

describe("detectThemeFromEnvironment", () => {
	it("returns light for COLORFGBG '0;15' (white bg, xterm index 15)", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "0;15" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("returns dark for COLORFGBG '15;0' (black bg, xterm index 0)", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "15;0" });
		expect(result).toEqual({ theme: "dark", source: "colorfgbg" });
	});

	it("uses final field for three-field format '0;0;15' (rxvt fg;bold;bg)", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "0;0;15" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("uses final field for three-field dark '15;0;0'", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "15;0;0" });
		expect(result).toEqual({ theme: "dark", source: "colorfgbg" });
	});

	it("returns undefined for invalid COLORFGBG (non-numeric)", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "foo;bar" })).toBeUndefined();
	});

	it("returns undefined for fractional final field", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "0;7.5" })).toBeUndefined();
	});

	it("returns undefined for out-of-range value (> 255)", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "0;256" })).toBeUndefined();
	});

	it("returns undefined for negative value", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "0;-1" })).toBeUndefined();
	});

	it("returns undefined for single-field COLORFGBG", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "15" })).toBeUndefined();
	});

	it("returns undefined for empty COLORFGBG", () => {
		expect(detectThemeFromEnvironment({ colorfgbg: "" })).toBeUndefined();
	});

	it("classifies xterm index 7 (light gray #c0c0c0) as light", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "0;7" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("classifies xterm index 8 (dark gray #808080) via luminance", () => {
		// #808080 has sRGB 0.502; luminance ≈ 0.216 → light (above 0.2 threshold)
		const result = detectThemeFromEnvironment({ colorfgbg: "0;8" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("classifies xterm 256 color cube dark index correctly", () => {
		// Index 16 = rgb(0,0,0) in the cube
		const result = detectThemeFromEnvironment({ colorfgbg: "15;16" });
		expect(result).toEqual({ theme: "dark", source: "colorfgbg" });
	});

	it("classifies xterm 256 color cube light index correctly", () => {
		// Index 231 = rgb(255,255,255) in the cube
		const result = detectThemeFromEnvironment({ colorfgbg: "0;231" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("classifies grayscale dark (index 232 = gray 8)", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "15;232" });
		expect(result).toEqual({ theme: "dark", source: "colorfgbg" });
	});

	it("classifies grayscale light (index 255 = gray 238)", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "0;255" });
		expect(result).toEqual({ theme: "light", source: "colorfgbg" });
	});

	it("returns light for Apple_Terminal when no COLORFGBG", () => {
		const result = detectThemeFromEnvironment({ termProgram: "Apple_Terminal" });
		expect(result).toEqual({ theme: "light", source: "apple-terminal" });
	});

	it("COLORFGBG takes precedence over Apple Terminal", () => {
		const result = detectThemeFromEnvironment({ colorfgbg: "15;0", termProgram: "Apple_Terminal" });
		expect(result).toEqual({ theme: "dark", source: "colorfgbg" });
	});

	it("returns undefined when no signals available", () => {
		expect(detectThemeFromEnvironment({})).toBeUndefined();
	});

	it("returns undefined with undefined env", () => {
		expect(detectThemeFromEnvironment()).toBeUndefined();
	});
});
