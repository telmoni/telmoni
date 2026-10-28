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
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { ALERTS_HEADING, MUTE_LENGTHS } from "@/lib/alerts";
import { useAction } from "@/lib/use-action";

import { useAct, useLanding } from "./_act";
import { endMuteAction, muteAction } from "./actions";

// A maintenance window mutes one alert, or every alert of the project, from
// now or a set time for a length the lane holds to thirty days: its incidents
// still open and resolve in the feed, and no connector hears them meanwhile.

const ROW_BUTTON = "h-9 gap-1.5 text-xs text-muted-foreground hover:text-foreground";

/** Opens the window's dialog: for the alert `ruleId`, or every alert. */
export function MuteButton({
  projectId,
  ruleId,
  name,
  disabled,
}: {
  projectId: string;
  ruleId: string | null;
  /** The alert's name; none for every alert. */
  name: string | null;
  disabled?: boolean;
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
        disabled={disabled}
        onClick={() => setOpen(true)}
        aria-label={name === null ? undefined : `Mute ${name}`}
      >
        {name === null ? "Mute every alert" : "Mute"}
      </Button>
      <MuteDialog
        key={`mute-${open}`}
        projectId={projectId}
        ruleId={ruleId}
        name={name}
        open={open}
        onOpenChange={setOpen}
        returnFocus={() => button.current?.focus()}
      />
    </>
  );
}

function MuteDialog({
  projectId,
  ruleId,
  name,
  open,
  onOpenChange,
  returnFocus,
}: {
  projectId: string;
  ruleId: string | null;
  name: string | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Opened by no trigger of its own, it hands the focus back by hand. */
  returnFocus: () => void;
}) {
  const id = useId();
  const [starts, setStarts] = useState<"now" | "later">("now");
  const [at, setAt] = useState("");
  const [minutes, setMinutes] = useState<number>(60);
  const { pending, error, setError, run } = useAction();
  // An alert's dialog goes with its card.
  const { gone } = useLanding(ALERTS_HEADING);

  function submit() {
    // The browser's own clock reads the time typed, a local one.
    const startsAt = starts === "later" ? new Date(at) : null;
    if (startsAt !== null && Number.isNaN(startsAt.getTime())) {
      setError("Set when the window starts.");
      return;
    }
    run(
      async () => {
        const r = await muteAction(projectId, {
          rule_id: ruleId,
          starts_at: startsAt?.toISOString() ?? null,
          minutes,
        });
        // What is gone takes the dialog with it.
        return gone(r) ? { ok: true, close: false } : r.ok ? { ok: true, close: true } : r;
      },
      ({ close }) => close && startTransition(() => onOpenChange(false)),
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
            <DialogTitle>{name === null ? "Mute every alert" : `Mute "${name}"`}</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              {name === null ? "Every alert of this project goes on" : "The alert goes on"} being
              evaluated, and its incidents open and resolve in the feed, but no connector hears
              them until the window ends.
            </DialogDescription>
            <div className="grid gap-2">
              <Label htmlFor={`${id}-starts`}>Starts</Label>
              <Select
                value={starts}
                onValueChange={(next) => {
                  setStarts(next as "now" | "later");
                  setError(null);
                }}
                disabled={pending}
              >
                <SelectTrigger id={`${id}-starts`} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="now">Now</SelectItem>
                  <SelectItem value="later">At a set time</SelectItem>
                </SelectContent>
              </Select>
            </div>
            {starts === "later" && (
              <div className="grid gap-2">
                <Label htmlFor={`${id}-at`}>Starts at</Label>
                <Input
                  id={`${id}-at`}
                  type="datetime-local"
                  value={at}
                  onChange={(e) => {
                    setAt(e.target.value);
                    setError(null);
                  }}
                  disabled={pending}
                />
              </div>
            )}
            <div className="grid gap-2">
              <Label htmlFor={`${id}-lasts`}>Lasts</Label>
              <Select
                value={String(minutes)}
                onValueChange={(next) => setMinutes(Number(next))}
                disabled={pending}
              >
                <SelectTrigger id={`${id}-lasts`} className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {MUTE_LENGTHS.map((length) => (
                    <SelectItem key={length.minutes} value={String(length.minutes)}>
                      {length.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button type="submit" disabled={pending}>
              {pending ? "Muting…" : "Mute"}
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

/** One of the project's windows ended now, the focus then going to the
 *  heading `landing` names. */
export function EndMute({
  projectId,
  muteId,
  label,
  landing,
}: {
  projectId: string;
  muteId: string;
  label: string;
  landing: string;
}) {
  const { pending, error, act } = useAct(landing);
  return (
    <span className="inline-flex flex-wrap items-center gap-3">
      <Button
        variant="outline"
        size="sm"
        className={ROW_BUTTON}
        disabled={pending}
        onClick={() => act(() => endMuteAction(projectId, muteId), true)}
        aria-label={label}
      >
        End
      </Button>
      {error && (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
    </span>
  );
}
