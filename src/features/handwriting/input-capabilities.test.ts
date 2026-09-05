import { describe, expect, it } from "vitest";
import { handwritingAvailable } from "./input-capabilities";

describe("handwriting visibility", () => {
  it("hides creation until a pen is detected, including on unknown desktop devices", () => {
    expect(handwritingAvailable("auto", "not_detected", false)).toBe(false);
    expect(handwritingAvailable("auto", "unknown", false)).toBe(false);
    expect(handwritingAvailable("auto", "available", false)).toBe(true);
    expect(handwritingAvailable("auto", "unknown", true)).toBe(true);
  });
  it("respects a local override without requiring a pen", () => {
    expect(handwritingAvailable("always", "not_detected", false)).toBe(true);
    expect(handwritingAvailable("hidden", "available", true)).toBe(false);
  });
});
