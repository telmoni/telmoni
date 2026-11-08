"use client";

import { startTransition, useId, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { CAP_FIELDS as FIELDS, capTyped } from "@/lib/limits";
import type { Caps } from "@/lib/server/entities/limits";
import { useAction } from "@/lib/use-action";

import { setProjectCapsAction } from "./actions";

const ROW_BUTTON = "text-muted-foreground hover:text-foreground";

/** Opens the dialog that sets one project's caps. */
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
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  return (
    <>
      <Button
        ref={button}
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        onClick={() => setOpen(true)}
        aria-label={`Set ${name}'s caps`}
      >
        Set caps
      </Button>
      <CapsDialog
        key={`caps-${open}`}
        organizationId={organizationId}
        projectId={projectId}
        name={name}
        caps={caps}
        open={open}
        onOpenChange={setOpen}
        returnFocus={() => button.current?.focus()}
      />
    </>
  );
}

function CapsDialog({
  organizationId,
  projectId,
  name,
  caps,
  open,
  onOpenChange,
  returnFocus,
}: {
  organizationId: string;
  projectId: string;
  name: string;
  caps: Caps;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  returnFocus: () => void;
}) {
  const id = useId();
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
      () => startTransition(() => onOpenChange(false)),
    );
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="sm:max-w-md"
        onCloseAutoFocus={(e) => {
          e.preventDefault();
          returnFocus();
        }}
      >
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (!pending) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>{`Caps on "${name}"`}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              What this project may use of the organization&apos;s pool, each at most the
              plan&apos;s. Leave one empty to share the pool. Past a cap, this project alone is
              held as the organization is past its plan.
            </DialogDescription>
            {FIELDS.map(({ key, label, unit }) => (
              <div key={key} className="grid gap-2">
                <Label htmlFor={`${id}-${key}`}>{label}</Label>
                <Input
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
              </div>
            ))}
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="submit" disabled={pending}>
              {pending ? "Saving…" : "Save caps"}
            </Button>
            <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
