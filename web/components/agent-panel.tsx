"use client";

import { useCallback, useEffect, useLayoutEffect, useReducer, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { usePathname } from "next/navigation";
import { ArrowUp, HistoryIcon, Info, MessageSquare, Plus, Square } from "lucide-react";

import {
  agentStatusAction,
  deleteAgentConversationAction,
  listAgentConversationsAction,
  openAgentConversationAction,
} from "@/app/(app)/agent-actions";
import { useConsoleUi } from "@/components/console-ui-context";
import { Conversation } from "@/components/agent/agent-messages";
import { Empty, HistoryList, ICON_BUTTON, type HistoryState } from "@/components/agent/agent-history-list";
import { Welcome } from "@/components/agent/agent-welcome";
import { AgentWindow } from "@/components/agent/agent-window";
import {
  agentEvents,
  agentReducer,
  describeRefusal,
  initialAgentState,
  isAgentDisabled,
} from "@/lib/agent/stream";
import { agentSuggestions } from "@/lib/agent/suggestions";
import { projectAt } from "@/lib/console-nav";
import { AGENT_MODIFIER_KEY, MODAL_SELECTOR } from "@/lib/keys";
import { useActiveOrganization, useProjects } from "@/lib/store";
import { AGENT_MESSAGE_MAX, type AgentStatus } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

type View = "conversation" | "history";

const PANEL_ID = "agent-panel";

// Within this of the end, a person is reading the end, and a reply that grows
// keeps it in view; further up, they are reading back, and are left there.
const PINNED_PX = 48;

// The composer grows with what is typed up to this, then scrolls.
const COMPOSER_MAX_PX = 160;

// How long a question gets before the composer counts it against the ceiling.
const COUNT_FROM = AGENT_MESSAGE_MAX - 500;

export function AgentPanel() {
  const { agentOpen, closeAgent, toggleAgent, openAgent, searchOpen, closeSearch } =
    useConsoleUi();
  const projects = useProjects();
  const organization = useActiveOrganization()?.slug ?? null;
  // The project the page stands in: none on the organization's own pages or
  // Account's.
  const pageProjectId = projectAt(usePathname(), organization, projects)?.id ?? null;

  const [view, setView] = useState<View>("conversation");
  const [state, dispatch] = useReducer(agentReducer, initialAgentState);
  const [history, setHistory] = useState<HistoryState>({ kind: "loading" });
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  // The project the conversation is about. A page of another project starts
  // clean rather than sending its questions into this one's; a page of none —
  // the organization's own, which a citation can lead to — keeps it, so the
  // window can be followed there and asked on. A project no longer listed
  // (moved, deleted, the organization switched) ends it. Reset while
  // rendering, as React has state follow a prop; the turn in flight is
  // aborted by the effect below.
  const [projectId, setProjectId] = useState(pageProjectId);
  const following =
    pageProjectId ?? (projects.some((p) => p.id === projectId) ? projectId : null);
  if (following !== projectId) {
    setProjectId(following);
    dispatch({ type: "reset" });
    setView("conversation");
    setHistory({ kind: "loading" });
    setNotice(null);
  }
  const project = projects.find((p) => p.id === projectId) ?? null;

  const [status, setStatus] = useState<{ projectId: string; status: AgentStatus } | null>(null);
  const [statusAsk, setStatusAsk] = useState(0);
  const effectiveStatus = projectId && status?.projectId === projectId ? status.status : null;

  const panelRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const columnRef = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  // The turn streaming now. ⚠ **Every way out of a turn aborts it**: a stop,
  // a new conversation, another one opened, another project, the window
  // closed. Left running, its events landed in the next turn's reply, and its
  // `done` handed the next question to the old conversation.
  const turnRef = useRef<AbortController | null>(null);
  // An answer that arrives after the person has moved on — another project,
  // another conversation opened or started — is dropped. Read through refs,
  // since the answer resolves after the render that asked.
  const shown = useRef({ projectId, conversationId: state.conversationId });
  const historyAsk = useRef(0);
  const openAsk = useRef(0);
  useEffect(() => {
    shown.current = { projectId, conversationId: state.conversationId };
  });

  const cancelTurn = useCallback(() => {
    turnRef.current?.abort();
    turnRef.current = null;
  }, []);
  useEffect(() => cancelTurn, [projectId, cancelTurn]);

  // Asked each time the window opens on a project, so a check that failed
  // once is not the answer for good, and a closed window asks nothing.
  useEffect(() => {
    if (!agentOpen || !projectId) return;
    let cancelled = false;
    void agentStatusAction(projectId).then((res) => {
      if (!cancelled) {
        setStatus({ projectId, status: "error" in res ? "unavailable" : res.status });
      }
    });
    return () => {
      cancelled = true;
    };
  }, [agentOpen, projectId, statusAsk]);

  // ⌘J opens and closes the window. The chord lives with the thing it opens,
  // as ⌘K does in `console-search.tsx`. From the search palette it hands
  // over, the two being the console's ways to ask it something; over any
  // other dialog or menu it does nothing, since the window would open behind
  // it, its field out of reach.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== AGENT_MODIFIER_KEY) return;
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.defaultPrevented || e.altKey || e.shiftKey) return;
      if (searchOpen) {
        e.preventDefault();
        closeSearch();
        openAgent();
        return;
      }
      if (document.querySelector(MODAL_SELECTOR)) return;
      e.preventDefault();
      toggleAgent();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [toggleAgent, openAgent, searchOpen, closeSearch]);

  // Focus on open, and Escape from inside the window closes it. Only from
  // inside: the window does not shut the page off, so Escape on the page, in
  // a dialog or in the search palette is theirs.
  useEffect(() => {
    if (!agentOpen) return;
    const previous = document.activeElement as HTMLElement | null;
    const timer = setTimeout(() => {
      inputRef.current?.focus();
      if (document.activeElement !== inputRef.current) panelRef.current?.focus();
    }, 50);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented) return;
      if (!panelRef.current?.contains(document.activeElement)) return;
      e.preventDefault();
      closeAgent();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", onKeyDown);
      previous?.focus();
    };
  }, [agentOpen, closeAgent]);

  // The field is disabled until the agent answers that it is there, so focus
  // opened on the window itself; it moves to the field once there is one.
  useEffect(() => {
    if (agentOpen && effectiveStatus === "enabled" && document.activeElement === panelRef.current) {
      inputRef.current?.focus();
    }
  }, [agentOpen, effectiveStatus]);

  // Closing stops the turn, and the model call behind it.
  useEffect(() => {
    if (agentOpen || !turnRef.current) return;
    cancelTurn();
    dispatch({ type: "stop" });
  }, [agentOpen, cancelTurn]);

  // The end of the conversation stays in view as it grows — a streaming
  // reply, a conversation opened, the window made larger — while the person
  // is reading the end of it.
  useEffect(() => {
    if (!agentOpen || view !== "conversation") return;
    const body = bodyRef.current;
    const column = columnRef.current;
    if (!body || !column) return;
    const stick = () => {
      if (pinned.current) body.scrollTop = body.scrollHeight;
    };
    stick();
    const observer = new ResizeObserver(stick);
    observer.observe(column);
    return () => observer.disconnect();
  }, [agentOpen, view]);

  useLayoutEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_PX)}px`;
  }, [draft, agentOpen]);

  const loadHistory = useCallback(async (pId: string) => {
    const ask = ++historyAsk.current;
    setHistory({ kind: "loading" });
    const res = await listAgentConversationsAction(pId);
    if (ask !== historyAsk.current || shown.current.projectId !== pId) return;
    setHistory(
      "error" in res
        ? { kind: "error", message: res.error }
        : { kind: "ok", conversations: res.conversations },
    );
  }, []);

  const openConversation = useCallback(
    async (conversationId: string) => {
      if (!projectId) return;
      const ask = ++openAsk.current;
      setNotice(null);
      const res = await openAgentConversationAction(projectId, conversationId);
      if (ask !== openAsk.current || shown.current.projectId !== projectId) return;
      if ("error" in res) {
        setNotice(res.error);
        return;
      }
      cancelTurn();
      pinned.current = true;
      dispatch({ type: "load", conversation: res.conversation });
      setView("conversation");
      inputRef.current?.focus();
    },
    [projectId, cancelTurn],
  );

  const deleteConversation = useCallback(
    async (conversationId: string) => {
      if (!projectId) return;
      const res = await deleteAgentConversationAction(projectId, conversationId);
      if (shown.current.projectId !== projectId) return;
      if (res.error) {
        setNotice(res.error);
        return;
      }
      if (shown.current.conversationId === conversationId) {
        cancelTurn();
        dispatch({ type: "reset" });
      }
      // The row goes now, rather than the whole list flashing back through
      // "loading", and focus, on the button that went with it, stays in the
      // window. ⚠ **Gone before focus moves, hence `flushSync`.** The confirm
      // hands focus back to its trigger as it finishes closing; left to a
      // later render, the row was sometimes still there to take it, and
      // focus fell to the page when the row went.
      flushSync(() =>
        setHistory((h) =>
          h.kind === "ok"
            ? { kind: "ok", conversations: h.conversations.filter((c) => c.id !== conversationId) }
            : h,
        ),
      );
      panelRef.current?.focus();
    },
    [projectId, cancelTurn],
  );

  const newConversation = useCallback(() => {
    openAsk.current++;
    cancelTurn();
    dispatch({ type: "reset" });
    setView("conversation");
    setNotice(null);
    inputRef.current?.focus();
  }, [cancelTurn]);

  const stop = useCallback(() => {
    cancelTurn();
    dispatch({ type: "stop" });
    inputRef.current?.focus();
  }, [cancelTurn]);

  const send = useCallback(
    async (raw: string) => {
      const text = raw.trim();
      if (!text || !projectId || state.streaming) return;
      openAsk.current++;
      setNotice(null);
      setView("conversation");
      pinned.current = true;
      // Back to the field for the next question: a starter's button goes
      // with the welcome it stood in, and focus would go with it.
      inputRef.current?.focus();

      const userId = crypto.randomUUID();
      const assistantId = crypto.randomUUID();
      dispatch({ type: "send", userId, assistantId, message: text });

      cancelTurn();
      const turn = new AbortController();
      turnRef.current = turn;
      // Only the turn the window is on may change it: one that was cancelled
      // (a stop, a reset, another project) can still have a dispatch in flight.
      const current = () => turnRef.current === turn;
      try {
        const response = await fetch("/api/agent/turns", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({
            projectId,
            conversationId: state.conversationId ?? undefined,
            message: text,
          }),
          signal: turn.signal,
        });

        if (!response.ok) {
          const body: unknown = await response.json().catch(() => null);
          if (!current()) return;
          if (isAgentDisabled(body)) setStatus({ projectId, status: "disabled" });
          dispatch({
            type: "fail",
            problem: describeRefusal(response.status, body, response.headers.get("Retry-After")),
          });
          return;
        }

        if (!response.body) {
          if (!current()) return;
          dispatch({
            type: "fail",
            problem: { title: "No response from agent", detail: null },
          });
          return;
        }

        for await (const event of agentEvents(response.body)) {
          if (!current()) return;
          dispatch({ type: "event", event });
          // Its last word: the turn is over, and closing the window now must
          // not stamp "stopped" on a finished reply.
          if (event.type === "done" || event.type === "error") {
            turnRef.current = null;
            return;
          }
        }
        // ⚠ **A stream can close without its last word**: the server's task
        // dying, a proxy cutting the connection. Without this the reply looked
        // in progress forever, and nothing more could be sent.
        if (current()) {
          dispatch({
            type: "fail",
            problem: {
              title: "The reply was cut off.",
              detail: "The connection closed before the agent finished. Try again.",
            },
          });
        }
      } catch {
        if (!current()) return;
        dispatch({
          type: "fail",
          problem: { title: "Network error", detail: "Please try again shortly." },
        });
      } finally {
        if (turnRef.current === turn) turnRef.current = null;
      }
    },
    [projectId, state.conversationId, state.streaming, cancelTurn],
  );

  const submit = () => {
    if (state.streaming || draft.trim() === "") return;
    void send(draft);
    setDraft("");
  };

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter while an input method is composing confirms the candidate; it
    // does not send a half-typed message. Safari reports that Enter with
    // `isComposing` already false, and keyCode 229 instead.
    const composing = e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229;
    if (e.key === "Enter" && !e.shiftKey && !composing) {
      e.preventDefault();
      submit();
    }
  };

  const toggleHistory = () => {
    if (view === "history") {
      setView("conversation");
    } else if (projectId) {
      void loadHistory(projectId);
      setView("history");
    }
  };

  const onBodyScroll = () => {
    const body = bodyRef.current;
    if (body) pinned.current = body.scrollHeight - body.scrollTop - body.clientHeight < PINNED_PX;
  };

  if (!agentOpen) return null;

  const canAsk = Boolean(projectId && effectiveStatus === "enabled");

  const title = (
    <>
      <span className="truncate">Telmoni Agent</span>
      <span
        className={cn(
          "inline-block size-2 shrink-0 rounded-full",
          effectiveStatus === "enabled"
            ? "bg-brand-positive"
            : effectiveStatus === null
              ? "bg-muted-foreground/40"
              : "bg-muted-foreground",
        )}
        aria-hidden
      />
    </>
  );

  const actions = (
    <>
      <button
        type="button"
        aria-label="New conversation"
        title="New conversation"
        disabled={!canAsk}
        onClick={newConversation}
        className={ICON_BUTTON}
      >
        <Plus className="size-4" />
      </button>
      <button
        type="button"
        aria-label={view === "history" ? "Back to the conversation" : "Conversation history"}
        title={view === "history" ? "Conversation" : "History"}
        disabled={!projectId}
        onClick={toggleHistory}
        className={ICON_BUTTON}
      >
        {view === "history" ? (
          <MessageSquare className="size-4" />
        ) : (
          <HistoryIcon className="size-4" />
        )}
      </button>
    </>
  );

  let content: React.ReactNode;
  if (!projectId) {
    content = <Notice>Open a project to ask the agent about it.</Notice>;
  } else if (effectiveStatus === null) {
    content = <Notice>Checking whether the agent is available…</Notice>;
  } else if (effectiveStatus === "disabled") {
    content = (
      <Notice>
        The agent isn&rsquo;t set up on this deployment. Ask whoever runs Telmoni here to
        configure it.
      </Notice>
    );
  } else if (effectiveStatus === "unavailable") {
    content = (
      <Notice
        action={
          <button
            type="button"
            onClick={() => {
              // The button goes as the check begins: focus waits on the
              // window, and moves to the field if the agent answers.
              panelRef.current?.focus();
              setStatus(null);
              setStatusAsk((n) => n + 1);
            }}
            className="cursor-pointer rounded-md border border-border px-3 py-1.5 text-sm hover:bg-accent"
          >
            Try again
          </button>
        }
      >
        The agent couldn&rsquo;t be reached.
      </Notice>
    );
  } else if (view === "history") {
    content = (
      <HistoryList
        history={history}
        activeId={state.conversationId}
        onOpen={(id) => void openConversation(id)}
        onDelete={(id) => void deleteConversation(id)}
      />
    );
  } else if (state.messages.length === 0 && !state.error) {
    content = (
      <Welcome
        project={project?.name ?? "this project"}
        suggestions={agentSuggestions(project?.role ?? null)}
        disabled={!canAsk || state.streaming}
        onAsk={(question) => void send(question)}
      />
    );
  } else {
    content = (
      <Conversation
        messages={state.messages}
        streaming={state.streaming}
        tool={state.tool}
        error={state.error}
        stopped={state.stopped}
      />
    );
  }

  return (
    <AgentWindow id={PANEL_ID} ref={panelRef} title={title} actions={actions} onClose={closeAgent}>
      <div
        ref={bodyRef}
        onScroll={onBodyScroll}
        className="flex min-h-0 flex-1 flex-col overflow-y-auto px-3.5"
      >
        {/* One column, as wide as reads well: all of a floating window, and
            the middle of the screen at full size. */}
        <div ref={columnRef} className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-4 py-3.5">
          {notice && (
            <p role="alert" className="text-sm text-destructive">
              {notice}
            </p>
          )}
          {content}
        </div>
      </div>

      <form
        className="shrink-0 px-3.5 pb-3.5"
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
            {/* Not disabled while a reply streams: a disabled field drops
                focus, which left the person typing into nothing. The next
                question can be written meanwhile, and sent once the reply is
                done. */}
            <textarea
              ref={inputRef}
              rows={1}
              value={draft}
              maxLength={AGENT_MESSAGE_MAX}
              disabled={!canAsk}
              aria-label="Ask the agent"
              placeholder={
                !projectId
                  ? "Open a project first"
                  : effectiveStatus === null
                    ? "One moment…"
                    : effectiveStatus !== "enabled"
                      ? "The agent is unavailable"
                      : state.messages.length > 0
                        ? "Reply…"
                        : "Ask about this project…"
              }
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={onInputKeyDown}
              className={cn(
                "block min-h-10 w-full resize-none border-0 bg-transparent px-3.5 pt-2.5 pb-1 text-sm outline-none",
                "placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50",
              )}
            />
            <div className="flex items-center justify-between gap-3 px-2 pb-2">
              <p className="flex min-w-0 items-center gap-1.5 pl-1.5 text-xs text-muted-foreground">
                <Info aria-hidden className="size-3.5 shrink-0" />
                <span className="truncate">
                  {project ? `Asking about ${project.name}` : "Asks about the project you open"}
                </span>
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
                aria-label={state.streaming ? "Stop the reply" : "Send"}
                title={state.streaming ? "Stop" : "Send"}
                disabled={!canAsk}
                aria-disabled={!state.streaming && draft.trim() === "" ? true : undefined}
                onClick={state.streaming ? stop : submit}
                className={cn(
                  "flex size-8 shrink-0 items-center justify-center rounded-md",
                  state.streaming
                    ? "cursor-pointer bg-foreground text-background hover:bg-foreground/80"
                    : canAsk && draft.trim() !== ""
                      ? "cursor-pointer bg-primary text-primary-foreground hover:bg-primary/90"
                      : "cursor-not-allowed bg-muted text-muted-foreground",
                )}
              >
                {state.streaming ? (
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
    </AgentWindow>
  );
}

// The window's word when there is nothing to converse in yet, in the middle of
// it, with the one thing to do about it where there is one.
function Notice({ children, action }: { children: React.ReactNode; action?: React.ReactNode }) {
  return (
    <div className="my-auto grid justify-items-center gap-3 text-center">
      <Empty>{children}</Empty>
      {action}
    </div>
  );
}
