"use client";

import { Square, SquareRoundCorner, type LucideIcon } from "lucide-react";

import { SegmentedSwitch } from "@/components/segmented-switch";
import type { Corners } from "@/lib/corners";
import { setCorners, useCorners } from "@/lib/use-corners";

// The same two choices the account menu offers, in the same order, so the
// switch in the footer and the switch in the menu are one control in two
// places.
const CHOICES = [
  { value: "rounded", label: "Rounded corners", Icon: SquareRoundCorner },
  { value: "sharp", label: "Sharp corners", Icon: Square },
] as const satisfies ReadonlyArray<{
  value: Corners;
  label: string;
  Icon: LucideIcon;
}>;

export function CornersToggle() {
  // No mounted guard, unlike `ThemeToggle`: the class on `<html>` IS the
  // state, the boot script puts it there before first paint, and
  // `useCorners`'s server snapshot is the default — so React swaps to the
  // real value at hydration and there is nothing to wait for.
  const corners = useCorners();

  return (
    <SegmentedSwitch
      label="Corners"
      options={CHOICES}
      value={corners}
      onChange={setCorners}
    />
  );
}
