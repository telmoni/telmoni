"use client";

import { Gauge } from "lucide-react";
import { startTransition, useId, useRef, useState } from "react";

import {
  SidePanel,
  SidePanelBody,
  SidePanelFooter,
  SidePanelHeader,
  SidePanelRow,
  usePanelOpening,
} from "@/components/side-panel";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { CAP_FIELDS as FIELDS, capTyped } from "@/lib/limits";
import type { Caps } from "@/lib/server/entities/limits";
import { useAction } from "@/lib/use-action";

import { setProjectCapsAction } from "./actions";

const ROW_BUTTON = "text-muted-foreground hover:text-foreground";

/** Opens the panel that sets one project's caps, as New project's opens. */
export function EditCaps({
  organizationId,
  projectId,
  name,
  caps,
}: {
  organizationId: string;
  projectId: string;
  name: string;
  caps: Caps;
}) {
  const { opening, open, close, place } = usePanelOpening();
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        onClick={(e) => open(e.currentTarget, null)}
        aria-label={`Set caps for ${name}`}
      >
        Set caps
      </Button>
      {place(
        opening && (
          <CapsPanel
            key={opening.at}
            organizationId={organizationId}
            projectId={projectId}
            name={name}
            caps={caps}
            onClose={close}
            returnFocus={() => opening.returnTo}
          />
        ),
      )}
    </>
  );
}

function CapsPanel({
  organizationId,
  projectId,
  name,
  caps,
  onClose,
  returnFocus,
}: {
  organizationId: string;
  projectId: string;
  name: string;
  caps: Caps;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
}) {
  const id = useId();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const first = useRef<HTMLInputElement>(null);
  const [typed, setTyped] = useState<Record<keyof Caps, string>>(() => {
    const shown = {} as Record<keyof Caps, string>;
    for (const { key, unit } of FIELDS) {
      const cap = caps[key];
      shown[key] = cap === null ? "" : String(unit ? cap / unit : cap);
    }
    return shown;
  });
  const { pending, error, setError, run } = useAction();

  function submit() {
    const next = {} as Caps;
    for (const field of FIELDS) {
      const cap = capTyped(field, typed[field.key]);
      if (cap !== null && typeof cap === "object") {
        setError(cap.refused);
        return;
      }
      next[field.key] = cap;
    }
    run(
      () => setProjectCapsAction(organizationId, projectId, next),
      () => startTransition(onClose),
    );
  }

  return (
    <SidePanel panel={panel} titleId={titleId} onClose={onClose} full initialFocus={first} returnFocus={returnFocus}>
      <form
        className="flex min-h-0 flex-1 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          if (!pending) submit();
        }}
      >
        <SidePanelHeader icon={Gauge} titleId={titleId} title={`Caps on "${name}"`} onClose={onClose}>
          <p className="text-sm text-muted-foreground">
            What this project may use of the organization&apos;s pool, each at most the plan&apos;s.
            Leave one empty to share the pool. Past a cap, this project alone is held as the
            organization is past its plan.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          {FIELDS.map(({ key, label, unit }, i) => (
            <SidePanelRow key={key} label={label} htmlFor={`${id}-${key}`}>
              <Input
                ref={i === 0 ? first : undefined}
                id={`${id}-${key}`}
                inputMode={unit ? "decimal" : "numeric"}
                placeholder="Shares the pool"
                value={typed[key]}
                onChange={(e) => {
                  setTyped({ ...typed, [key]: e.target.value });
                  setError(null);
                }}
                disabled={pending}
              />
            </SidePanelRow>
          ))}
          {error && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" variant="filled" disabled={pending}>
            {pending ? "Saving…" : "Save caps"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
  );
}
