import { describe, expect, it } from "vitest";

import { decimalTyped, wholeTyped } from "./typed-number";

describe("typed numbers", () => {
  it("keeps a whole number's digits alone", () => {
    expect(wholeTyped("56kkk")).toBe("56");
    expect(wholeTyped("1,440 min")).toBe("1440");
    expect(wholeTyped("-5")).toBe("5");
  });

  it("keeps a decimal's digits, one point and grouping commas, and a minus only where it is signed", () => {
    expect(decimalTyped("3.5abc")).toBe("3.5");
    expect(decimalTyped("1,000.25")).toBe("1,000.25");
    expect(decimalTyped("1.2.3")).toBe("1.23");
    expect(decimalTyped("-20")).toBe("20");
    expect(decimalTyped("-20", true)).toBe("-20");
    expect(decimalTyped("2-0", true)).toBe("20");
  });
});
