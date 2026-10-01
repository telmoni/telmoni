import { describe, expect, it } from "vitest";

import { escapeGuestText } from "./guest-text";

const RLO = "\u202E";
const LRM = "\u200E";
const ESC = "\u001B";

const BIDI = [
  "\u061C",
  "\u200E",
  "\u200F",
  "\u202A",
  "\u202B",
  "\u202C",
  "\u202D",
  "\u202E",
  "\u2066",
  "\u2067",
  "\u2068",
  "\u2069",
];

describe("guest text", () => {
  it("defuses a name that lies about its own extension", () => {
    const shown = escapeGuestText(`report${RLO}gnp.exe`);
    expect(shown).toBe("report<U+202E>gnp.exe");
    expect(shown).not.toContain(RLO);
  });

  it("escapes every bidirectional control, not just the override", () => {
    expect(BIDI).toHaveLength(12);
    for (const ch of BIDI) {
      expect(escapeGuestText(`a${ch}b`)).not.toContain(ch);
    }
    expect(escapeGuestText(`a${LRM}b`)).toBe("a<U+200E>b");
  });

  it("escapes the controls that rewrite what came before", () => {
    expect(escapeGuestText(`done${ESC}[2Kfailed`)).toBe(
      "done<U+001B>[2Kfailed",
    );
    expect(escapeGuestText("a\u0000b")).toBe("a<U+0000>b");
    expect(escapeGuestText("a\u007Fb")).toBe("a<U+007F>b");
    expect(escapeGuestText("a\u0085b")).toBe("a<U+0085>b");
  });

  it("keeps the three whitespace controls output is made of", () => {
    expect(escapeGuestText("one\ntwo\tthree\rfour")).toBe(
      "one\ntwo\tthree\rfour",
    );
  });

  it("escapes the separators that add a line", () => {
    expect(escapeGuestText("a\u2028b")).toBe("a<U+2028>b");
    expect(escapeGuestText("a\u2029b")).toBe("a<U+2029>b");
  });

  it("leaves ordinary text, and every script, alone", () => {
    for (const s of [
      "hello",
      "مرحبا",
      "日本語",
      "emoji \u{1F389}",
      "<script>",
    ]) {
      expect(escapeGuestText(s)).toBe(s);
    }
  });

  it("is idempotent, and cannot be imitated", () => {
    const once = escapeGuestText(`x${RLO}y`);
    expect(escapeGuestText(once)).toBe(once);
    expect(escapeGuestText("<U+202E>")).toBe("<U+202E>");
  });
});
