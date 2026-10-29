"use client";

import { useLayoutEffect, type KeyboardEvent, type RefObject } from "react";
import { ArrowUp, Info, Square } from "lucide-react";

import { AGENT_MESSAGE_MAX } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

// The composer grows with what is typed up to this, then scrolls.
const MAX_PX = 160;

// How long a question gets before the composer counts it against the ceiling.
const COUNT_FROM = AGENT_MESSAGE_MAX - 500;

/** The window's field and its one button, Send or Stop: `disabled` while the
 *  agent is not there to ask. Not disabled while a reply streams: a disabled
 *  field drops focus, which left the person typing into nothing. The next
 *  question can be written meanwhile, and sent once the reply is done. */
export function Composer({
  inputRef,
  draft,
  onDraft,
  placeholder,
  about,
  disabled,
  streaming,
  onSend,
  onStop,
}: {
  inputRef: RefObject<HTMLTextAreaElement | null>;
  draft: string;
  onDraft: (next: string) => void;
  placeholder: string;
  /** What a question is about, under the field. */
  about: string;
  disabled: boolean;
  streaming: boolean;
  onSend: () => void;
  onStop: () => void;
}) {
  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, MAX_PX)}px`;
  }, [draft, inputRef]);

  const submit = () => {
    if (streaming || draft.trim() === "") return;
    onSend();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter while an input method is composing confirms the candidate; it
    // does not send a half-typed message. Safari reports that Enter with
    // `isComposing` already false, and keyCode 229 instead.
    const composing = e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229;
    if (e.key === "Enter" && !e.shiftKey && !composing) {
      e.preventDefault();
      submit();
    }
  };

  return (
    <form
      className="shrink-0 px-gutter pb-gutter"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <div className="mx-auto w-full max-w-3xl">
        <div
          className={cn(
            "rounded-lg border border-input bg-transparent",
            "dark:bg-input/30 dark:border-transparent",
            "focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50",
            "transition-[color,box-shadow]",
          )}
        >
          <textarea
            ref={inputRef}
            rows={1}
            value={draft}
            maxLength={AGENT_MESSAGE_MAX}
            disabled={disabled}
            aria-label="Ask the agent"
            placeholder={placeholder}
            onChange={(e) => onDraft(e.target.value)}
            onKeyDown={onKeyDown}
            className={cn(
              "block min-h-10 w-full resize-none border-0 bg-transparent px-gutter pt-2.5 pb-1 text-sm outline-none",
              "placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50",
            )}
          />
          <div className="flex items-center justify-between gap-3 px-2 pb-2">
            <p className="flex min-w-0 items-center gap-1.5 pl-1.5 text-xs text-muted-foreground">
              <Info aria-hidden className="size-3.5 shrink-0" />
              <span className="truncate">{about}</span>
            </p>
            {/* `maxLength` cuts a paste past the ceiling short without a
                word, so the count shows before it does. */}
            {draft.length >= COUNT_FROM && (
              <span className="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">
                {draft.length.toLocaleString()} / {AGENT_MESSAGE_MAX.toLocaleString()}
              </span>
            )}
            {/* One button, Send or Stop, so focus on it survives the turn
                starting and ending: two would swap under it, and a removed
                or disabled button drops focus. An empty field marks it
                `aria-disabled` rather than disabling it, for the same
                reason. A plain button, never a submit one: Stop must not
                become a submit while its own press is still landing. */}
            <button
              type="button"
              aria-label={streaming ? "Stop the reply" : "Send"}
              title={streaming ? "Stop" : "Send"}
              disabled={disabled}
              aria-disabled={!streaming && draft.trim() === "" ? true : undefined}
              onClick={streaming ? onStop : submit}
              className={cn(
                "flex size-8 shrink-0 items-center justify-center rounded-md",
                streaming
                  ? "cursor-pointer bg-foreground text-background hover:bg-foreground/80"
                  : !disabled && draft.trim() !== ""
                    ? "cursor-pointer bg-primary text-primary-foreground hover:bg-primary/90"
                    : "cursor-not-allowed bg-muted text-muted-foreground",
              )}
            >
              {streaming ? (
                <Square className="size-3 fill-current" />
              ) : (
                <ArrowUp className="size-4" />
              )}
            </button>
          </div>
        </div>
        <p className="pt-2.5 text-center text-[11px] text-muted-foreground">
          Telmoni Agent is AI and can make mistakes.
        </p>
      </div>
    </form>
  );
}
