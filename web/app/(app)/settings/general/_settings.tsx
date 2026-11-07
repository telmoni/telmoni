"use client";

import { useSyncExternalStore } from "react";
import { useTheme } from "next-themes";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { CORNERS_DEFAULT, isCorners, type Corners } from "@/lib/corners";
import { isOneOf } from "@/lib/one-of";
import { DENSITIES, type Density } from "@/lib/trace-prefs";
import { setCorners, useCorners } from "@/lib/use-corners";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";

type ThemeChoice = "system" | "light" | "dark";

const THEMES: readonly Choice<ThemeChoice>[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const CORNER_CHOICES: readonly Choice<Corners>[] = [
  { value: "rounded", label: "Rounded" },
  { value: "sharp", label: "Sharp" },
];

const DENSITY_CHOICES: readonly Choice<Density>[] = [
  { value: "comfortable", label: "Comfortable" },
  { value: "compact", label: "Compact" },
];

const noop = () => () => {};

// The theme is read from storage on the client alone, so the server and the
// first client render both show System until the page has mounted.
function useMounted(): boolean {
  return useSyncExternalStore(noop, () => true, () => false);
}

function isThemeChoice(value: string | undefined): value is ThemeChoice {
  return value === "system" || value === "light" || value === "dark";
}

export function GeneralSettings() {
  const { theme, setTheme } = useTheme();
  const mounted = useMounted();
  const corners = useCorners();
  const { density } = useTracePrefs();
  const shown = mounted && isThemeChoice(theme) ? theme : "system";

  return (
    <div className="grid gap-page">
      <Section card title="Appearance" description="How the console looks. Saved in this browser.">
        <Rows flush>
          <Row
            label={<span id="general-theme-label">Theme</span>}
            hint="System follows your device's light or dark setting."
          >
            <ChoiceGroup
              labelledBy="general-theme-label"
              options={THEMES}
              value={shown}
              onChange={setTheme}
            />
          </Row>
          <Row
            label={<span id="general-corners-label">Corners</span>}
            hint="Rounded or square edges on cards, fields and buttons."
          >
            <ChoiceGroup
              labelledBy="general-corners-label"
              options={CORNER_CHOICES}
              value={corners ?? CORNERS_DEFAULT}
              onChange={(value) => {
                if (isCorners(value)) setCorners(value);
              }}
            />
          </Row>
        </Rows>
      </Section>
      <Section card
        title="Lists"
        description="How the Traces, Sessions and Users lists look. Saved in this browser."
      >
        <Rows flush>
          <Row
            label={<span id="general-density-label">Row density</span>}
            hint="Compact fits more rows on the screen."
          >
            <ChoiceGroup
              labelledBy="general-density-label"
              options={DENSITY_CHOICES}
              value={density}
              onChange={(value) => {
                if (isOneOf(DENSITIES, value)) setTracePrefs({ density: value });
              }}
            />
          </Row>
        </Rows>
      </Section>
    </div>
  );
}
