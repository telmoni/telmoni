"use client";

import { useId, useState } from "react";
import { toast } from "sonner";

import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { CodeField, CodeSentTo } from "@/components/code-field";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { CONTENT_MODE_LABEL as LABELS } from "@/lib/content-mode";
import { ContentMode } from "@/lib/types/enums";
import { useAction } from "@/lib/use-action";
import { useCodeConfirmed } from "@/lib/use-code-confirmed";
import { cn } from "@/lib/utils";

import { requestContentModeCodeAction, updateContentModeAction } from "./actions";

/** "On", or "Off and On": the modes named in a sentence. */
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
  email,
}: {
  projectId: string;
  /** The project's mode, as the server answered it. */
  mode: ContentMode;
  /** The modes the server lets the switch choose. */
  offered: readonly ContentMode[];
  /** The organization's owner alone may change it. */
  canEdit: boolean;
  /** The owner's address, where a move out of `off` mails its code. */
  email: string;
}) {
  const labelId = useId();
  const noteId = useId();
  const deleteId = useId();
  const codeId = useId();
  const [choice, setChoice] = useState<ContentMode>(mode);
  const [deleteContent, setDeleteContent] = useState(false);
  const { pending, error, run } = useAction();
  // Leaving `off` is confirmed from the owner's inbox: the code is mailed,
  // then typed, and the switch spends it.
  const flow = useCodeConfirmed(
    () => requestContentModeCodeAction(projectId),
    async (code) => {
      const r = await updateContentModeAction(projectId, choice, { code });
      if (r.ok) toast.success(`Content mode set to ${LABELS[choice]}.`);
      return r;
    },
  );

  // Anyone but the owner reads the mode as text: a group of closed radios
  // is dimmed and out of the tab order, which is no way to show an answer.
  if (!canEdit) {
    return (
      <div className="grid gap-gutter text-sm">
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
  const leavingOff = isChanged && mode === ContentMode.Off;
  const goingOff = isChanged && choice === ContentMode.Off;
  // Marked, never `disabled`: a button disabled under the focus drops it to
  // the page, and Save is pressed, then held while it saves, then has
  // nothing left to save. The guard below is what refuses the press.
  const held = !isChanged || pending;

  const handleSubmit = (e: React.SubmitEvent<HTMLFormElement>) => {
    e.preventDefault();
    if (held) return;
    // A refusal the server gave also redraws the page (the action's
    // `refresh()`), since it can mean the page is out of date; a change, the
    // action's revalidation sends back with the new mode.
    if (leavingOff) {
      // Enter in the code's field confirms it; before then, the press sends it.
      if (flow.step.step !== "code") flow.sendCode();
      else if (flow.canConfirm) flow.confirmCode();
      return;
    }
    run(
      () => updateContentModeAction(projectId, choice, { deleteContent: goingOff && deleteContent }),
      () =>
        toast.success(
          goingOff && deleteContent
            ? "Content mode set to Off, and what the project kept is deleted."
            : `Content mode set to ${LABELS[choice]}.`,
        ),
    );
  };

  const saveLabel = pending
    ? "Saving..."
    : flow.sending
      ? "Sending..."
      : leavingOff && flow.step.step !== "code"
        ? "Send confirmation code"
        : "Save";
  const shownError = error ?? (leavingOff ? flow.error : null);

  return (
    <form onSubmit={handleSubmit} className="grid gap-gutter">
      <div className="flex flex-wrap items-center gap-gutter">
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
          aria-disabled={held || flow.sending}
          className={cn((held || flow.sending) && "cursor-not-allowed opacity-50")}
        >
          {saveLabel}
        </Button>
      </div>
      {goingOff && (
        <div className="grid gap-1">
          <Label htmlFor={deleteId} className="font-normal">
            <Checkbox
              id={deleteId}
              checked={deleteContent}
              onCheckedChange={(on) => setDeleteContent(on === true)}
              disabled={pending}
            />
            Also delete what this project kept
          </Label>
          <p className="text-sm text-muted-foreground">
            {deleteContent
              ? "Its key is deleted with it at once, so nothing reads it again; a backup taken before keeps a copy until it expires. There is no undo, and you are mailed a receipt."
              : "What it kept stays readable to its members, each read recorded, until its spans' retention ends."}
          </p>
        </div>
      )}
      {leavingOff && flow.step.step === "code" && (
        <div className="grid gap-gutter">
          <CodeField
            id={codeId}
            label={<CodeSentTo email={email} />}
            value={flow.code}
            onType={flow.type}
            disabled={flow.confirming}
          />
          <div className="flex items-center gap-gutter">
            <Button type="submit" disabled={!flow.canConfirm}>
              {flow.confirming ? "Saving..." : "Keep content"}
            </Button>
            <Button
              type="button"
              variant="outline"
              disabled={flow.sending || flow.confirming}
              onClick={flow.sendCode}
            >
              Resend code
            </Button>
          </div>
        </div>
      )}
      {shownError && (
        <p className="text-sm text-destructive" role="alert">
          {shownError}
        </p>
      )}
      {unoffered.length > 0 && (
        <p id={noteId} className="text-sm text-muted-foreground">
          {named(unoffered)} {unoffered.length === 1 ? "is" : "are"} not available yet.
        </p>
      )}
    </form>
  );
}
