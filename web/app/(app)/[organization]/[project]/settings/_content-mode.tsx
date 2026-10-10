"use client";

import { useRouter } from "next/navigation";
import { useId, useState, useTransition } from "react";
import { toast } from "sonner";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { Button } from "@/components/ui/button";
import { ContentMode } from "@/lib/types/enums";
import { cn } from "@/lib/utils";

import { updateContentModeAction } from "./actions";

const LABELS: Record<ContentMode, string> = {
  [ContentMode.Off]: "Off",
  [ContentMode.On]: "On",
  [ContentMode.Sealed]: "Sealed",
};

/** "On", "On and Sealed", "On, Sealed and Off": the modes named in a sentence. */
function named(modes: ContentMode[]): string {
  const labels = modes.map((m) => LABELS[m]);
  const last = labels.pop();
  return labels.length ? `${labels.join(", ")} and ${last}` : (last ?? "");
}

export function ContentModeForm({
  projectId,
  mode,
  offered,
  canEdit,
}: {
  projectId: string;
  /** The project's mode, as the server answered it. */
  mode: ContentMode;
  /** The modes the server lets the switch choose. */
  offered: readonly ContentMode[];
  /** The organization's owner alone may change it. */
  canEdit: boolean;
}) {
  const router = useRouter();
  const labelId = useId();
  const noteId = useId();
  const [choice, setChoice] = useState<ContentMode>(mode);
  const [error, setError] = useState<string | null>(null);
  const [pending, startTransition] = useTransition();

  // A change from another tab arrives as a new `mode` on refresh: the choice
  // follows it, and an unsaved choice is not kept against a mode that moved,
  // nor against one the server has stopped offering since it was picked.
  const [lastMode, setLastMode] = useState(mode);
  if (mode !== lastMode) {
    setLastMode(mode);
    setChoice(mode);
  } else if (choice !== mode && !offered.includes(choice)) {
    setChoice(mode);
  }

  // Anyone but the owner reads the mode as text: a group of closed radios
  // is dimmed and out of the tab order, which is no way to show an answer.
  if (!canEdit) {
    return (
      <div className="grid gap-3 text-sm">
        <p>
          <span className="font-medium">Content mode:</span> {LABELS[mode]}
        </p>
        <p className="text-muted-foreground">
          Only the organization owner can change what this project keeps.
        </p>
      </div>
    );
  }

  const modes = Object.values(ContentMode);
  const options: Choice<ContentMode>[] = modes.map((m) => ({
    value: m,
    label: LABELS[m],
    disabled: pending || !offered.includes(m),
  }));
  const unoffered = modes.filter((m) => !offered.includes(m));
  const isChanged = choice !== mode && offered.includes(choice);
  // Marked, never `disabled`: a button disabled under the focus drops it to
  // the page, and Save is pressed, then held while it saves, then has
  // nothing left to save. The guard below is what refuses the press.
  const held = !isChanged || pending;

  const handleSubmit = (e: React.SubmitEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (held) return;

    setError(null);
    startTransition(async () => {
      try {
        const res = await updateContentModeAction(projectId, choice);
        if (res.error) {
          setError(res.error);
          toast.error(res.error);
          // A refusal can mean the page is out of date — the owner changed,
          // or a mode closed — so it is drawn again from the server.
          router.refresh();
        } else {
          // The action's revalidation sends the page back with the new mode.
          toast.success(`Content mode set to ${LABELS[choice]}.`);
        }
      } catch {
        const msg = "Network error. Please try again.";
        setError(msg);
        toast.error(msg);
      }
    });
  };

  return (
    <form onSubmit={handleSubmit} className="grid gap-3">
      <div className="flex flex-wrap items-center gap-3">
        <span id={labelId} className="text-sm font-medium">
          Content mode
        </span>
        <ChoiceGroup
          labelledBy={labelId}
          describedBy={unoffered.length > 0 ? noteId : undefined}
          options={options}
          value={choice}
          onChange={setChoice}
        />
        <Button
          type="submit"
          aria-disabled={held}
          className={cn(held && "cursor-not-allowed opacity-50")}
        >
          {pending ? "Saving..." : "Save"}
        </Button>
      </div>
      {error && <p className="text-sm text-destructive">{error}</p>}
      {unoffered.length > 0 && (
        <p id={noteId} className="text-sm text-muted-foreground">
          {named(unoffered)} {unoffered.length === 1 ? "is" : "are"} not available yet.
        </p>
      )}
    </form>
  );
}
