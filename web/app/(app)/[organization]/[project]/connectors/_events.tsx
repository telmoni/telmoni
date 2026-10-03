"use client";

import { useId } from "react";

import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { NOTIFICATION_KINDS, eventKindLabel } from "@/lib/notification-kinds";
import type { NotificationKind } from "@/lib/types/enums";

// What sets each kind off, in the owner's terms. Keyed by the generated
// vocabulary for the same reason the labels are: a new kind has no row here
// until someone says what raises it.
const HINT = {
  organization_alert: "An alert for the organization's owner and admins.",
  member_added: "Someone accepted an invitation to this project.",
  member_left: "Someone left or was removed from this project.",
  connector_connected: "A destination was connected to this project.",
  connector_disconnected: "A destination stopped accepting notices.",
} as const satisfies Record<NotificationKind, string>;

// The picker's own state. "Selected" with nothing chosen is a state the
// owner passes through while choosing, so it is representable here and
// refused at submit, not prevented by the control.
export interface EventChoice {
  all: boolean;
  kinds: NotificationKind[];
}

export function choiceFrom(eventKinds: NotificationKind[] | null): EventChoice {
  return eventKinds === null ? { all: true, kinds: [] } : { all: false, kinds: eventKinds };
}

// `null` is every kind, the ones a later release adds included — which a
// list of today's kinds, however complete, is not.
export function kindsFrom(choice: EventChoice): NotificationKind[] | null {
  return choice.all ? null : NOTIFICATION_KINDS.filter((k) => choice.kinds.includes(k));
}

export function isChoiceEmpty(choice: EventChoice): boolean {
  return !choice.all && choice.kinds.length === 0;
}

export function EventPicker({
  value,
  onChange,
  disabled,
}: {
  value: EventChoice;
  onChange: (next: EventChoice) => void;
  disabled?: boolean;
}) {
  const id = useId();
  return (
    <div className="grid gap-2">
      <Label htmlFor={`${id}-mode`}>Events</Label>
      <Select
        value={value.all ? "all" : "selected"}
        onValueChange={(mode) => onChange({ ...value, all: mode === "all" })}
        disabled={disabled}
      >
        <SelectTrigger id={`${id}-mode`} className="w-full">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="all">All events</SelectItem>
          <SelectItem value="selected">Selected events</SelectItem>
        </SelectContent>
      </Select>
      {value.all ? (
        <p className="text-xs text-muted-foreground">
          Every kind of notice, including kinds added later.
        </p>
      ) : (
        <ul className="grid gap-3 rounded-menu border border-border p-3">
          {NOTIFICATION_KINDS.map((kind) => {
            const switchId = `${id}-${kind}`;
            const checked = value.kinds.includes(kind);
            return (
              <li key={kind} className="flex items-start justify-between gap-4">
                <div className="grid gap-0.5">
                  <Label htmlFor={switchId}>{eventKindLabel(kind)}</Label>
                  <p id={`${switchId}-hint`} className="text-xs text-muted-foreground">
                    {HINT[kind]}
                  </p>
                </div>
                <Switch
                  id={switchId}
                  checked={checked}
                  onCheckedChange={(on) =>
                    onChange({
                      ...value,
                      kinds: on
                        ? [...value.kinds, kind]
                        : value.kinds.filter((k) => k !== kind),
                    })
                  }
                  disabled={disabled}
                  aria-describedby={`${switchId}-hint`}
                />
              </li>
            );
          })}
        </ul>
      )}
      {isChoiceEmpty(value) && (
        <p className="text-xs text-muted-foreground">Choose at least one event.</p>
      )}
    </div>
  );
}
