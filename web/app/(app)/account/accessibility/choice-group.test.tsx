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
