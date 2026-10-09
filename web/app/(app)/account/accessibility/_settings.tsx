"use client";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { useConsoleUi } from "@/components/console-ui-context";
import { Row, Rows } from "@/components/rows";
import { Section } from "@/components/section";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { ContrastChoice, MotionChoice } from "@/lib/accessibility";
import {
  setContrast,
  setFocusRings,
  setLetterKeys,
  setMotion,
  useContrast,
  useFocusRings,
  useLetterKeys,
  useMotion,
  useSystemReducesMotion,
  useSystemWantsMoreContrast,
} from "@/lib/use-accessibility";

// "On" and "Off" are the person's own word; "System" hands the choice to the
// device's setting, and the row says what the device asks for right now.
const MOTION: readonly Choice<MotionChoice>[] = [
  { value: "system", label: "System" },
  { value: "reduce", label: "On" },
  { value: "allow", label: "Off" },
];

const CONTRAST: readonly Choice<ContrastChoice>[] = [
  { value: "system", label: "System" },
  { value: "more", label: "On" },
  { value: "standard", label: "Off" },
];

export function AccessibilitySettings({ supportEmail }: { supportEmail: string | null }) {
  const letterKeys = useLetterKeys();
  const focusRings = useFocusRings();
  const motion = useMotion();
  const contrast = useContrast();
  const deviceReducesMotion = useSystemReducesMotion();
  const deviceWantsContrast = useSystemWantsMoreContrast();
  const { openShortcuts } = useConsoleUi();

  return (
    <div className="grid gap-6">
      <Section title="Keyboard" description="How the console answers the keyboard.">
        <Rows>
          <Row
            label={<Label htmlFor="a11y-letter-keys">Letter-key shortcuts</Label>}
            hint="Single keys such as g, c and / move around and act. Turn them off if you use voice control or a screen reader, so a dictated word never opens a page; shortcuts held with ⌘ or Ctrl keep working."
          >
            <Switch id="a11y-letter-keys" checked={letterKeys} onCheckedChange={setLetterKeys} />
          </Row>
          <Row
            label={<Label htmlFor="a11y-focus-rings">Show focus</Label>}
            hint="Draw a ring around whatever has keyboard focus, so you can see where you are. It shows when you move with the keyboard and in a field you type in, never when you click a button or a link."
          >
            <Switch id="a11y-focus-rings" checked={focusRings} onCheckedChange={setFocusRings} />
          </Row>
          <Row label="Keyboard shortcuts" hint="Every shortcut that works where you are.">
            <Button type="button" variant="outline" size="sm" onClick={openShortcuts}>
              Show
            </Button>
          </Row>
        </Rows>
      </Section>

      <Section title="Display" description="How the interface moves, and how much it stands out.">
        <Rows>
          <Row
            label={<span id="a11y-motion-label">Reduce motion</span>}
            hint={
              <>
                Menus, the sidebar and dialogs appear at once instead of sliding and fading.
                {motion === "system" && (
                  <>
                    {" "}
                    System follows your device, which{" "}
                    {deviceReducesMotion ? "asks for less motion now." : "does not ask for less motion now."}
                  </>
                )}
              </>
            }
          >
            <ChoiceGroup
              labelledBy="a11y-motion-label"
              options={MOTION}
              value={motion}
              onChange={setMotion}
            />
          </Row>
          <Row
            label={<span id="a11y-contrast-label">Increase contrast</span>}
            hint={
              <>
                Darker secondary text, and clear edges on fields, cards and dividers.
                {contrast === "system" && (
                  <>
                    {" "}
                    System follows your device, which{" "}
                    {deviceWantsContrast ? "asks for more contrast now." : "does not ask for more contrast now."}
                  </>
                )}
              </>
            }
          >
            <ChoiceGroup
              labelledBy="a11y-contrast-label"
              options={CONTRAST}
              value={contrast}
              onChange={setContrast}
            />
          </Row>
        </Rows>
      </Section>

      <p className="text-xs text-muted-foreground">
        Saved in this browser, as the theme and corners are, and applied to the console, the docs
        and the site.
        {supportEmail && (
          <>
            {" "}
            Something hard to reach or to read? Write to{" "}
            <a
              href={`mailto:${supportEmail}`}
              className="underline underline-offset-2 hover:text-foreground"
            >
              {supportEmail}
            </a>{" "}
            and say what you browse with.
          </>
        )}
      </p>
    </div>
  );
}
