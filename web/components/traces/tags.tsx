"use client";

import { useId, useRef, useState } from "react";
import { PlusIcon, XIcon } from "lucide-react";
import { toast } from "sonner";

import {
  setTraceTagsAction,
  tagTracesAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { formatCount, tagKeys, tagValues } from "@/lib/traces";
import { useToastAction } from "@/lib/use-action";

// A run's tags changed by hand, as any member of the project may: each one
// taken off at its ×, and one added from a small form whose fields suggest
// the keys the window's tags hold and the values of the key typed. The
// server holds every rule a tag keeps; its refusal is the toast.

/** A run's tags, each with its ×, and the form that adds one; `onChange` is
 *  told once a change lands, for a read the page's refresh does not reach. */
export function TraceTags({
  projectId,
  runId,
  tags,
  known,
  onChange,
}: {
  projectId: string;
  runId: string;
  tags: Readonly<Record<string, string>>;
  /** The window's tags, each `key:value`. */
  known: readonly string[];
  onChange?: () => void;
}) {
  const { pending, run } = useToastAction();
  const [open, setOpen] = useState(false);
  const adder = useRef<HTMLButtonElement>(null);
  const change = (body: { add?: Record<string, string>; remove?: string[] }, then: () => void) => {
    if (pending) return;
    run(
      () => setTraceTagsAction(projectId, runId, body),
      () => {
        then();
        onChange?.();
      },
    );
  };
  return (
    <>
      {Object.entries(tags).map(([key, value]) => (
        <Badge key={key} variant="secondary" className="gap-0.5 pr-0.5 text-xs font-normal">
          {key}={value}
          <button
            type="button"
            aria-label={`Remove tag ${key}`}
            aria-disabled={pending || undefined}
            // Its chip goes with it, so the focus waits on Add tag.
            onClick={() => change({ remove: [key] }, () => adder.current?.focus())}
            className="inline-flex size-4 cursor-pointer items-center justify-center rounded-sm text-muted-foreground outline-none hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:ring-inset aria-disabled:cursor-not-allowed"
          >
            <XIcon className="size-3" />
          </button>
        </Badge>
      ))}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            ref={adder}
            variant="ghost"
            size="xs"
            className="text-muted-foreground hover:text-foreground"
          >
            <PlusIcon />
            Add tag
          </Button>
        </PopoverTrigger>
        <PopoverContent align="start" aria-label="Add tag" className="w-64 p-gutter">
          <TagForm
            known={known}
            label="Add tag"
            pending={pending}
            onAdd={(key, value) => change({ add: { [key]: value } }, () => setOpen(false))}
          />
        </PopoverContent>
      </Popover>
    </>
  );
}

/** A tag set on every trace a session's page shows, one after another: how
 *  many took it, or where it stopped, is the toast. */
export function TagEveryTrace({
  projectId,
  runIds,
  known,
}: {
  projectId: string;
  runIds: readonly string[];
  /** The tags its traces carry, each `key:value`. */
  known: readonly string[];
}) {
  const { pending, run } = useToastAction();
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="outline">
          <PlusIcon />
          Add tag to every trace
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        aria-label="Add tag to every trace"
        className="grid w-64 gap-gutter p-gutter"
      >
        <p className="text-xs text-muted-foreground">
          {runIds.length === 1
            ? "The trace shown takes it."
            : `Each of the ${formatCount(runIds.length)} traces shown takes it, one after another.`}
        </p>
        <TagForm
          known={known}
          label="Add tag to every trace"
          pending={pending}
          onAdd={(key, value) =>
            run(
              () => tagTracesAction(projectId, runIds, { key, value }),
              ({ tagged }) => {
                setOpen(false);
                toast.success(`Tagged ${formatCount(tagged)} ${tagged === 1 ? "trace" : "traces"}`);
              },
            )
          }
        />
      </PopoverContent>
    </Popover>
  );
}

/** A key and a value asked for, the fields suggesting the keys `known`
 *  holds and the values of the key typed. */
function TagForm({
  known,
  label,
  pending,
  onAdd,
}: {
  known: readonly string[];
  /** What its button says. */
  label: string;
  pending: boolean;
  onAdd: (key: string, value: string) => void;
}) {
  const id = useId();
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const trimmed = key.trim();
  return (
    <form
      className="grid gap-gutter"
      onSubmit={(event) => {
        event.preventDefault();
        if (trimmed) onAdd(trimmed, value.trim());
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor={`${id}key`}>Key</Label>
        <Input
          id={`${id}key`}
          list={`${id}keys`}
          value={key}
          onChange={(event) => setKey(event.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
        <datalist id={`${id}keys`}>
          {tagKeys(known).map((each) => (
            <option key={each} value={each} />
          ))}
        </datalist>
      </div>
      <div className="grid gap-2">
        <Label htmlFor={`${id}value`}>Value</Label>
        <Input
          id={`${id}value`}
          list={`${id}values`}
          value={value}
          onChange={(event) => setValue(event.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
        <datalist id={`${id}values`}>
          {tagValues(known, trimmed).map((each) => (
            <option key={each} value={each} />
          ))}
        </datalist>
      </div>
      <Button type="submit" disabled={pending || !trimmed}>
        {label}
      </Button>
    </form>
  );
}
