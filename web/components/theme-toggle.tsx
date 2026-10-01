"use client";

import { Monitor, Moon, Sun } from "lucide-react";
import { useTheme } from "next-themes";
import { useSyncExternalStore } from "react";

import { SegmentedSwitch } from "@/components/segmented-switch";

const subscribeToNothing = () => () => {};
const onClient = () => true;
const onServer = () => false;

const MODES = [
  { value: "system", label: "System theme", Icon: Monitor },
  { value: "light", label: "Light theme", Icon: Sun },
  { value: "dark", label: "Dark theme", Icon: Moon },
] as const;

export function ThemeToggle() {
  const { theme, setTheme } = useTheme();
  // `next-themes` reports no theme until it has read storage on the client, so
  // the server renders a switch with nothing pressed and the first client
  // paint must agree. `null` until mounted says "not known yet" rather than
  // pressing a default the next paint would move.
  const mounted = useSyncExternalStore(subscribeToNothing, onClient, onServer);

  return (
    <SegmentedSwitch
      label="Theme"
      options={MODES}
      value={mounted ? (theme ?? null) : null}
      onChange={setTheme}
    />
  );
}
