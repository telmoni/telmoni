"use client";

import { BellOff } from "lucide-react";
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

const ROW_BUTTON = "text-muted-foreground hover:text-foreground";

/** Opens the window's panel: for the alert `ruleId`, or every alert. */
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
  const { opening, open, close, place } = usePanelOpening();
  return (
    <>
      <Button
        variant="outline"
        size="sm"
        // Every alert's sits beside the page's actions, and takes their type.
        className={name === null ? "text-muted-foreground hover:text-foreground" : ROW_BUTTON}
        disabled={disabled}
        onClick={(e) => open(e.currentTarget, null)}
        aria-label={name === null ? undefined : `Mute ${name}`}
      >
        {name === null ? "Mute every alert" : "Mute"}
      </Button>
      {place(
        opening && (
          <MutePanel
            key={opening.at}
            projectId={projectId}
            ruleId={ruleId}
            name={name}
            onClose={close}
            returnFocus={() => opening.returnTo}
          />
        ),
      )}
    </>
  );
}

function MutePanel({
  projectId,
  ruleId,
  name,
  onClose,
  returnFocus,
}: {
  projectId: string;
  ruleId: string | null;
  name: string | null;
  onClose: () => void;
  returnFocus: () => HTMLElement | null;
}) {
  const id = useId();
  const titleId = useId();
  const panel = useRef<HTMLDivElement>(null);
  const first = useRef<HTMLButtonElement>(null);
  const [starts, setStarts] = useState<"now" | "later">("now");
  const [at, setAt] = useState("");
  const [minutes, setMinutes] = useState<number>(60);
  const { pending, error, setError, run } = useAction();
  // An alert's panel goes with its card.
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
        // What is gone takes the panel with it.
        return gone(r) ? { ok: true, close: false } : r.ok ? { ok: true, close: true } : r;
      },
      ({ close }) => close && startTransition(onClose),
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
        <SidePanelHeader
          icon={BellOff}
          titleId={titleId}
          title={name === null ? "Mute every alert" : `Mute "${name}"`}
          onClose={onClose}
        >
          <p className="text-sm text-muted-foreground">
            {name === null ? "Every alert of this project goes on" : "The alert goes on"} being
            evaluated, and its incidents open and resolve in the feed, but no connector hears
            them until the window ends.
          </p>
        </SidePanelHeader>
        <SidePanelBody>
          <SidePanelRow label="Starts" htmlFor={`${id}-starts`}>
            <Select
              value={starts}
              onValueChange={(next) => {
                setStarts(next as "now" | "later");
                setError(null);
              }}
              disabled={pending}
            >
              <SelectTrigger ref={first} id={`${id}-starts`} className="w-full max-w-md">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="now">Now</SelectItem>
                <SelectItem value="later">At a set time</SelectItem>
              </SelectContent>
            </Select>
          </SidePanelRow>
          {starts === "later" && (
            <SidePanelRow label="Starts at" htmlFor={`${id}-at`}>
              <Input
                id={`${id}-at`}
                type="datetime-local"
                className="max-w-md"
                value={at}
                onChange={(e) => {
                  setAt(e.target.value);
                  setError(null);
                }}
                disabled={pending}
              />
            </SidePanelRow>
          )}
          <SidePanelRow label="Lasts" htmlFor={`${id}-lasts`}>
            <Select
              value={String(minutes)}
              onValueChange={(next) => setMinutes(Number(next))}
              disabled={pending}
            >
              <SelectTrigger id={`${id}-lasts`} className="w-full max-w-md">
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
          </SidePanelRow>
          {error && (
            <p className="py-2 text-sm text-destructive" role="alert">
              {error}
            </p>
          )}
        </SidePanelBody>
        <SidePanelFooter>
          <Button type="submit" variant="filled" disabled={pending}>
            {pending ? "Muting…" : "Mute"}
          </Button>
          <Button type="button" variant="outline" onClick={onClose}>
            Cancel
          </Button>
        </SidePanelFooter>
      </form>
    </SidePanel>
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
    <span className="inline-flex flex-wrap items-center gap-2">
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
