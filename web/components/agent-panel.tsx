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
  initialAgentState,
  type AgentProblem,
} from "@/lib/agent/stream";
import { isOrganizationSegment, rootSegment } from "@/lib/console-nav";
import { useProjects } from "@/lib/store";
import { AGENT_MESSAGE_MAX, type AgentStatus } from "@/lib/types/agent";
import { cn } from "@/lib/utils";

type View = "conversation" | "history";

const PANEL_ID = "agent-panel";

export function AgentPanel() {
  const { agentOpen, closeAgent } = useConsoleUi();
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

  const [status, setStatus] = useState<AgentStatus | null>(null);
  const [view, setView] = useState<View>("conversation");
  const [state, dispatch] = useReducer(agentReducer, initialAgentState);
  const [history, setHistory] = useState<HistoryState>({ kind: "loading" });
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [, startTransition] = useTransition();

  const panelRef = useRef<HTMLElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  // Track status per project so navigating between enabled and disabled
  // projects updates the badge and form controls.
  useEffect(() => {
    if (!projectId) return;
    let cancelled = false;
    void agentStatusAction(projectId).then((res) => {
      if (!cancelled) {
        if ("error" in res) {
          setStatus("unavailable");
        } else {
          setStatus(res.status);
        }
      }
    });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  // Focus trap / escape handling.
  useEffect(() => {
    if (!agentOpen) return;
    const previous = document.activeElement as HTMLElement | null;
    const timer = setTimeout(() => {
      inputRef.current?.focus();
      if (document.activeElement !== inputRef.current) panelRef.current?.focus();
    }, 50);

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        closeAgent();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("keydown", onKeyDown);
      previous?.focus();
    };
  }, [agentOpen, closeAgent]);

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
      dispatch({ type: "load", conversation: res.conversation });
      setView("conversation");
      inputRef.current?.focus();
    },
    [projectId],
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
        dispatch({ type: "reset" });
      }
      void loadHistory(projectId);
    },
    [projectId, state.conversationId, loadHistory],
  );

  const newConversation = useCallback(() => {
    dispatch({ type: "reset" });
    setView("conversation");
    setNotice(null);
    inputRef.current?.focus();
  }, []);

  const send = useCallback(async () => {
    const text = draft.trim();
    if (!text || !projectId || state.streaming) return;
    setDraft("");
    setNotice(null);

    const userId = crypto.randomUUID();
    const assistantId = crypto.randomUUID();
    dispatch({ type: "send", userId, assistantId, message: text });

    try {
      const response = await fetch("/api/agent/turns", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          projectId,
          conversationId: state.conversationId ?? undefined,
          message: text,
        }),
      });

      if (!response.ok) {
        let problem: AgentProblem = {
          title: "Couldn't talk to the agent",
          detail: "Try again shortly.",
        };
        try {
          const data = await response.json();
          if (data.title) {
            problem = { title: data.title, detail: data.detail ?? null };
          }
        } catch {
          // ignore
        }
        dispatch({ type: "fail", problem });
        return;
      }

      if (!response.body) {
        dispatch({
          type: "fail",
          problem: { title: "No response from agent", detail: null },
        });
        return;
      }

      for await (const event of agentEvents(response.body)) {
        dispatch({ type: "event", event });
      }
    } catch {
      dispatch({
        type: "fail",
        problem: { title: "Network error", detail: "Please try again shortly." },
      });
    }
  }, [draft, projectId, state.conversationId, state.streaming]);

  const onInputKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
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

  const effectiveStatus = projectId ? status : null;
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
        "flex min-h-0 min-w-0 flex-col overflow-hidden outline-none",
        "rounded-t-console-surface bg-console-surface",
        "max-lg:absolute max-lg:inset-y-0 max-lg:inset-x-3.5 max-lg:z-10",
        "lg:flex-1",
      )}
    >
      <div className="flex min-h-14 shrink-0 items-center gap-3 px-4 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-sm font-semibold">Telmoni Agent</span>
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
          <p className="truncate text-xs text-muted-foreground">
            {!projectId
              ? "Select a project to start"
              : effectiveStatus === "enabled"
                ? "Answers questions about your telemetry"
                : effectiveStatus === "disabled"
                  ? "Agent is disabled"
                  : effectiveStatus === "unavailable"
                    ? "Agent unavailable"
                    : "Connecting…"}
          </p>
        </div>

        <div className="flex items-center gap-1">
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

      <div ref={bodyRef} className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto p-4">
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
        className="flex min-h-14 shrink-0 items-center justify-end gap-2 px-4 py-2.5"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
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
          className="h-8 w-11"
          aria-label="Send"
          title="Send"
          disabled={!canSend || draft.trim() === ""}
        >
          <ArrowUp />
        </Button>
      </form>
    </aside>
  );
}
