// @vitest-environment jsdom
import { fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, it } from "vitest";

import { ChoiceGroup } from "@/components/choice-group";

const OPTIONS = [
  { value: "system", label: "System" },
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
] as const;

function Harness() {
  const [value, setValue] = useState<(typeof OPTIONS)[number]["value"]>("system");
  return (
    <>
      <span id="name">Reduce motion</span>
      <ChoiceGroup labelledBy="name" options={OPTIONS} value={value} onChange={setValue} />
    </>
  );
}

describe("ChoiceGroup", () => {
  it("is a named radio group with exactly one radio checked", () => {
    render(<Harness />);
    const group = screen.getByRole("radiogroup", { name: "Reduce motion" });
    expect(group).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "System" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("aria-checked", "false");
  });

  // WAI-ARIA's radio pattern: one stop on Tab, the checked radio.
  it("puts only the checked radio in the tab order", () => {
    render(<Harness />);
    expect(screen.getByRole("radio", { name: "System" })).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("tabindex", "-1");
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("tabindex", "-1");
  });

  it("moves the choice and the focus with the arrow keys, wrapping, and with Home and End", () => {
    render(<Harness />);
    const group = screen.getByRole("radiogroup");
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("aria-checked", "true");
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "On" }));
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "Home" });
    expect(screen.getByRole("radio", { name: "System" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "End" });
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
  });

  it("chooses on a click", () => {
    render(<Harness />);
    fireEvent.click(screen.getByRole("radio", { name: "On" }));
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "System" })).toHaveAttribute("aria-checked", "false");
  });
});

const MODES = [
  { value: "off", label: "Off" },
  { value: "on", label: "On", disabled: true },
  { value: "sealed", label: "Sealed" },
] as const;

function ModesHarness({ initial }: { initial: (typeof MODES)[number]["value"] }) {
  const [value, setValue] = useState<(typeof MODES)[number]["value"]>(initial);
  return (
    <>
      <span id="mode">Content mode</span>
      <ChoiceGroup labelledBy="mode" options={MODES} value={value} onChange={setValue} />
    </>
  );
}

describe("ChoiceGroup with a choice that is not open", () => {
  it("draws it disabled, and a click does not choose it", () => {
    render(<ModesHarness initial="off" />);
    const on = screen.getByRole("radio", { name: "On" });
    expect(on).toBeDisabled();
    fireEvent.click(on);
    expect(on).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
  });

  // As the arrow keys pass over a disabled radio.
  it("passes over it with the arrow keys, Home and End", () => {
    render(<ModesHarness initial="off" />);
    const group = screen.getByRole("radiogroup", { name: "Content mode" });
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(screen.getByRole("radio", { name: "Sealed" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "Sealed" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "Home" });
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("aria-checked", "true");
    fireEvent.keyDown(group, { key: "End" });
    expect(screen.getByRole("radio", { name: "Sealed" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("aria-checked", "false");
  });

  // A checked radio that cannot take the focus would leave the group with no
  // Tab stop at all.
  it("puts the first open choice in the tab order when the checked one is closed", () => {
    render(<ModesHarness initial="on" />);
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Off" })).toHaveAttribute("tabindex", "0");
    expect(screen.getByRole("radio", { name: "On" })).toHaveAttribute("tabindex", "-1");
    expect(screen.getByRole("radio", { name: "Sealed" })).toHaveAttribute("tabindex", "-1");
  });

  // Closed choices at the ends, as the owner's content modes are: Home and
  // End land on the open ends, and the arrows wrap past the closed ones.
  it("lands Home and End on the open ends, and wraps past closed ones", () => {
    const EDGES = [
      { value: "a", label: "A", disabled: true },
      { value: "b", label: "B" },
      { value: "c", label: "C" },
      { value: "d", label: "D", disabled: true },
    ] as const;
    function EdgesHarness() {
      const [value, setValue] = useState<(typeof EDGES)[number]["value"]>("b");
      return (
        <>
          <span id="edges">Edges</span>
          <ChoiceGroup labelledBy="edges" options={EDGES} value={value} onChange={setValue} />
        </>
      );
    }
    render(<EdgesHarness />);
    const group = screen.getByRole("radiogroup", { name: "Edges" });
    const checked = () => screen.getByRole("radio", { checked: true }).textContent;
    fireEvent.keyDown(group, { key: "End" });
    expect(checked()).toBe("C");
    fireEvent.keyDown(group, { key: "Home" });
    expect(checked()).toBe("B");
    fireEvent.keyDown(group, { key: "ArrowLeft" });
    expect(checked()).toBe("C");
    fireEvent.keyDown(group, { key: "ArrowRight" });
    expect(checked()).toBe("B");
  });

  // The arrow keys move from where the focus is, which is not the checked
  // radio when that one is closed.
  it("moves from the focused choice when the checked one is closed", () => {
    render(<ModesHarness initial="on" />);
    const off = screen.getByRole("radio", { name: "Off" });
    off.focus();
    fireEvent.keyDown(off, { key: "ArrowLeft" });
    expect(screen.getByRole("radio", { name: "Sealed" })).toHaveAttribute("aria-checked", "true");
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "Sealed" }));
  });
});
