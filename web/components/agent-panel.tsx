"use client";

import { useCallback, useEffect, useReducer, useRef, useState, useTransition } from "react";
import { useParams } from "next/navigation";
import { ArrowUp, HistoryIcon, MessageSquare, SquarePen, X as XIcon } from "lucide-react";

import {
  agentStatusAction,
  deleteAgentConversationAction,
  listAgentConversationsAction,
  openAgentConversationAction,
} from "@/app/(app)/agent-actions";
import { useConsoleUi } from "@/components/console-ui-context";
import { Conversation } from "@/components/agent/agent-messages";
import { Empty, HistoryList, ICON_BUTTON, type HistoryState } from "@/components/agent/agent-history-list";
import { Button } from "@/components/ui/button";
import {
  agentEvents,
  agentReducer,
  describeRefusal,
  initialAgentState,
  isAgentDisabled,
} from "@/lib/agent/stream";
import { isOrganizationSegment, rootSegment } from "@/lib/console-nav";
import { AGENT_MODIFIER_KEY } from "@/lib/keys";
import { useProjects } from "@/lib/store";
import { AGENT_MESSAGE_MAX, type AgentStatus } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

type View = "conversation" | "history";

const PANEL_ID = "agent-panel";

export function AgentPanel() {
  const { agentOpen, closeAgent, toggleAgent } = useConsoleUi();
  const { projectId: routeProjectId } = useParams<{ projectId?: string }>();
  const projects = useProjects();
  const rawProjectId = typeof routeProjectId === "string" ? routeProjectId : null;
  // An organization segment is not a project; the agent requires a project.
  const projectId =
    rawProjectId &&
    !isOrganizationSegment(rootSegment(rawProjectId)) &&
    projects.some((p) => p.id === rawProjectId)
      ? rawProjectId
      : null;

  const [status, setStatus] = useState<{ projectId: string; status: AgentStatus } | null>(null);
  const [view, setView] = useState<View>("conversation");
  const [state, dispatch] = useReducer(agentReducer, initialAgentState);
  const [history, setHistory] = useState<HistoryState>({ kind: "loading" });
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [, startTransition] = useTransition();

  const panelRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  // The turn streaming now. ⚠ **Every way out of a turn aborts it**: a new
  // conversation, another one opened, another project, the panel closed.
  // Left running, its events landed in the next turn's reply, and its `done`
  // handed the next question to the old conversation.
  const turnRef = useRef<AbortController | null>(null);

  const cancelTurn = useCallback(() => {
    turnRef.current?.abort();
    turnRef.current = null;
  }, []);

  // Asked each time the panel opens on a project, so a check that failed
  // once is not the answer for good, and a closed panel asks nothing.
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
  }, [agentOpen, projectId]);

  // A conversation belongs to the project it was asked in; another project
  // starts clean rather than sending its questions into this one's. Reset
  // while rendering, as React has state follow a prop; the turn in flight is
  // aborted by the effect below.
  const [shownProjectId, setShownProjectId] = useState(projectId);
  if (shownProjectId !== projectId) {
    setShownProjectId(projectId);
    dispatch({ type: "reset" });
    setView("conversation");
    setHistory({ kind: "loading" });
    setNotice(null);
  }
  useEffect(() => cancelTurn, [projectId, cancelTurn]);

  // ⌘J opens and closes the panel. The chord lives with the thing it opens,
  // as ⌘K does in `console-search.tsx`.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() !== AGENT_MODIFIER_KEY) return;
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.defaultPrevented || e.altKey || e.shiftKey) return;
      e.preventDefault();
      toggleAgent();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [toggleAgent]);

  // Focus on open, and Escape from inside the panel closes it. Only from
  // inside: Escape in a dialog or the search palette closes that alone.
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

  // Closing stops the turn, and the model call behind it.
  useEffect(() => {
    if (agentOpen || !turnRef.current) return;
    cancelTurn();
    dispatch({
      type: "fail",
      problem: { title: "Stopped when the agent was closed.", detail: null },
    });
  }, [agentOpen, cancelTurn]);

  // Auto-scroll on new messages / tokens.
  useEffect(() => {
    if (view !== "conversation") return;
    const el = bodyRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [state.messages, state.streaming, view]);

  const loadHistory = useCallback(async (pId: string) => {
    setHistory({ kind: "loading" });
    const res = await listAgentConversationsAction(pId);
    if ("error" in res) {
      setHistory({ kind: "error", message: res.error });
    } else {
      setHistory({ kind: "ok", conversations: res.conversations });
    }
  }, []);

  const openConversation = useCallback(
    async (conversationId: string) => {
      if (!projectId) return;
      setNotice(null);
      const res = await openAgentConversationAction(projectId, conversationId);
      if ("error" in res) {
        setNotice(res.error);
        return;
      }
      cancelTurn();
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
      if (res.error) {
        setNotice(res.error);
        return;
      }
      if (state.conversationId === conversationId) {
        cancelTurn();
        dispatch({ type: "reset" });
      }
      void loadHistory(projectId);
    },
    [projectId, state.conversationId, loadHistory, cancelTurn],
  );

  const newConversation = useCallback(() => {
    cancelTurn();
    dispatch({ type: "reset" });
    setView("conversation");
    setNotice(null);
    inputRef.current?.focus();
  }, [cancelTurn]);

  const send = useCallback(async () => {
    const text = draft.trim();
    if (!text || !projectId || state.streaming) return;
    setDraft("");
    setNotice(null);

    const userId = crypto.randomUUID();
    const assistantId = crypto.randomUUID();
    dispatch({ type: "send", userId, assistantId, message: text });

    cancelTurn();
    const turn = new AbortController();
    turnRef.current = turn;
    // Only the turn the panel is on may change it: one that was cancelled
    // (a reset, another project) can still have a dispatch in flight.
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
        // Its last word: the turn is over, and closing the panel now must
        // not stamp "stopped" on a finished reply.
        if (event.type === "done" || event.type === "error") {
          turnRef.current = null;
          return;
        }
      }
      // ⚠ **A stream can close without its last word**: the server's task
      // dying, a proxy cutting the connection. Without this the reply looked
      // in progress forever and the input stayed disabled.
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
  }, [draft, projectId, state.conversationId, state.streaming, cancelTurn]);

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Enter while an input method is composing confirms the candidate; it
    // does not send a half-typed message. Safari reports that Enter with
    // `isComposing` already false, and keyCode 229 instead.
    const composing = e.nativeEvent.isComposing || e.nativeEvent.keyCode === 229;
    if (e.key === "Enter" && !e.shiftKey && !composing) {
      e.preventDefault();
      void send();
    }
  };

  const toggleHistory = () => {
    if (view === "history") {
      setView("conversation");
    } else if (projectId) {
      startTransition(() => {
        void loadHistory(projectId);
        setView("history");
      });
    }
  };

  const onClose = () => closeAgent();

  if (!agentOpen) return null;

  const effectiveStatus = projectId && status?.projectId === projectId ? status.status : null;
  const canSend = Boolean(projectId && effectiveStatus === "enabled" && !state.streaming);

  return (
    <aside
      id={PANEL_ID}
      ref={panelRef}
      tabIndex={-1}
      aria-label="Telmoni Agent"
      // A surface of its own beside the page's: from `lg` an equal half of
      // the row (`flex-1`, as the page's surface is), below `lg` a cover laid
      // exactly over the page's surface (the shell's `px-3.5`), never past its
      // edge.
      className={cn(
        "flex min-h-0 min-w-0 flex-col gap-3 px-4 pt-4 pb-0 outline-none overflow-hidden",
        "rounded-t-console-surface bg-console-surface",
        "max-lg:absolute max-lg:inset-y-0 max-lg:inset-x-3.5 max-lg:z-10",
        "lg:flex-1",
      )}
    >
      <div className="flex min-h-8 shrink-0 items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-base font-medium">Telmoni Agent</span>
          <span
            className={cn(
              "inline-block size-2 rounded-full",
              effectiveStatus === "enabled"
                ? "bg-brand-positive"
                : effectiveStatus === null
                  ? "bg-muted-foreground/40"
                  : "bg-muted-foreground",
            )}
            aria-hidden
          />
        </div>

        <div className="flex items-center gap-3">
          <button
            type="button"
            aria-label={view === "history" ? "Back to conversation" : "Conversation history"}
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
          <button
            type="button"
            aria-label="New conversation"
            title="New conversation"
            disabled={!projectId || effectiveStatus !== "enabled"}
            onClick={newConversation}
            className={ICON_BUTTON}
          >
            <SquarePen className="size-4" />
          </button>
          <button
            type="button"
            aria-label="Close the agent"
            title="Close"
            onClick={onClose}
            className={ICON_BUTTON}
          >
            <XIcon className="size-4" />
          </button>
        </div>
      </div>

      <div ref={bodyRef} className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto">
        {notice && (
          <p role="alert" className="text-sm text-destructive">
            {notice}
          </p>
        )}
        {!projectId ? (
          <Empty>Open a project to ask the agent about it.</Empty>
        ) : effectiveStatus === null ? (
          <Empty>Checking whether the agent is available…</Empty>
        ) : effectiveStatus === "disabled" ? (
          <Empty>
            The agent isn&rsquo;t set up on this deployment. Ask whoever runs Telmoni here to
            configure it.
          </Empty>
        ) : effectiveStatus === "unavailable" ? (
          <Empty>The agent couldn&rsquo;t be reached. Close this and try again shortly.</Empty>
        ) : view === "history" ? (
          <HistoryList
            history={history}
            activeId={state.conversationId}
            onOpen={(id) => void openConversation(id)}
            onDelete={(id) => void deleteConversation(id)}
          />
        ) : (
          <Conversation
            messages={state.messages}
            streaming={state.streaming}
            tool={state.tool}
            error={state.error}
          />
        )}
      </div>

      <form
        className="flex shrink-0 flex-col"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <div className="flex items-center gap-2">
          <textarea
            ref={inputRef}
            rows={1}
            value={draft}
            maxLength={AGENT_MESSAGE_MAX}
            disabled={!canSend}
            aria-label="Ask the agent"
            placeholder={
              !projectId
                ? "Open a project first"
                : effectiveStatus === "enabled"
                  ? "Ask about this project…"
                  : "Agent is unavailable"
            }
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onInputKeyDown}
            className={cn(
              "min-h-8 max-h-32 min-w-0 flex-1 resize-none rounded-menu border border-input",
              "bg-transparent px-3 py-1.5 text-sm outline-none placeholder:text-muted-foreground",
              "focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50",
            )}
          />
          <Button
            type="submit"
            size="icon"
            className="size-8 shrink-0"
            aria-label="Send"
            title="Send"
            disabled={!canSend || draft.trim() === ""}
          >
            <ArrowUp />
          </Button>
        </div>
        <p className="text-center text-[11px] text-muted-foreground py-3">
          Telmoni Agent is AI and can make mistakes.
        </p>
      </form>
    </aside>
  );
}
