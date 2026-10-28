import { describe, expect, it } from "vitest";

import { CONFIRMATION_CODE_LENGTH, codeTyped, isConfirmationCode } from "./confirmation-code";

describe("a confirmation code", () => {
  it("is exactly its length in digits", () => {
    expect(isConfirmationCode("012345")).toBe(true);
    expect(isConfirmationCode("12345")).toBe(false);
    expect(isConfirmationCode("1234567")).toBe(false);
    expect(isConfirmationCode("12 345")).toBe(false);
    expect(isConfirmationCode("")).toBe(false);
  });

  it("is typed as its digits alone, cut to its length", () => {
    expect(codeTyped("12-34 56")).toBe("123456");
    expect(codeTyped("1234567890")).toHaveLength(CONFIRMATION_CODE_LENGTH);
    expect(codeTyped("abc")).toBe("");
  });
});
