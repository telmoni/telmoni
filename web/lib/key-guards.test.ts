// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import { inFront, isChord, typedInto } from "./keys";

afterEach(() => {
  document.body.innerHTML = "";
});

function keyOn(target: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, ...init });
  Object.defineProperty(event, "target", { value: target });
  return event;
}

describe("typedInto", () => {
  it("is a field, a list, an editable region and a panel's form", () => {
    document.body.innerHTML = `
      <input id="field" />
      <div role="listbox"><span id="option"></span></div>
      <div contenteditable="true"><span id="prose"></span></div>
      <div role="dialog"><form><button id="toggle"></button></form></div>
      <button id="page"></button>`;
    for (const id of ["field", "option", "prose", "toggle"]) {
      expect(typedInto(keyOn(document.getElementById(id)!)), id).toBe(true);
    }
    expect(typedInto(keyOn(document.getElementById("page")!))).toBe(false);
  });

  // An input method's keys spell a word, whatever has the focus.
  it("is a key an input method is still composing", () => {
    document.body.innerHTML = `<button id="page"></button>`;
    expect(typedInto(keyOn(document.getElementById("page")!, { isComposing: true }))).toBe(true);
  });
});

describe("inFront", () => {
  it("is a modal dialog, an alert or a menu, never the agent's window", () => {
    expect(inFront()).toBe(false);
    document.body.innerHTML = `<div role="dialog" aria-modal="false"></div>`;
    expect(inFront()).toBe(false);
    for (const html of [
      `<div role="dialog"></div>`,
      `<div role="alertdialog"></div>`,
      `<div role="menu"></div>`,
    ]) {
      document.body.innerHTML = html;
      expect(inFront(), html).toBe(true);
    }
  });
});

describe("isChord", () => {
  const body = () => document.body;

  it("is ⌘ or Ctrl with the key, Shift as asked", () => {
    expect(isChord(keyOn(body(), { key: "k", metaKey: true }), "k")).toBe(true);
    expect(isChord(keyOn(body(), { key: "K", ctrlKey: true }), "k")).toBe(true);
    expect(isChord(keyOn(body(), { key: "K", metaKey: true, shiftKey: true }), "k", true)).toBe(true);
    expect(isChord(keyOn(body(), { key: "k", metaKey: true, shiftKey: true }), "k")).toBe(false);
    expect(isChord(keyOn(body(), { key: "k" }), "k")).toBe(false);
    expect(isChord(keyOn(body(), { key: "j", metaKey: true }), "k")).toBe(false);
  });

  it("is not a chord with Alt, or one a page already answered", () => {
    expect(isChord(keyOn(body(), { key: "k", metaKey: true, altKey: true }), "k")).toBe(false);
    const answered = keyOn(body(), { key: "k", metaKey: true, cancelable: true });
    answered.preventDefault();
    expect(isChord(answered, "k")).toBe(false);
  });
});
