"use client";

import { useEffect, useReducer, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { unstable_rethrow, usePathname } from "next/navigation";
import { HistoryIcon, MessageSquare, Plus } from "lucide-react";

import { deleteAgentConversationAction } from "@/app/(app)/agent-actions";
import { useConsoleUi } from "@/components/console-ui-context";
import { Conversation } from "@/components/agent/agent-messages";
import { Empty, HistoryList, type HistoryState } from "@/components/agent/agent-history-list";
import { Welcome } from "@/components/agent/agent-welcome";
import { AgentWindow } from "@/components/agent/agent-window";
import { Composer } from "@/components/agent/composer";
import { readAgentConversation, readAgentConversations } from "@/components/agent/reads";
import { useAgentStatus } from "@/components/agent/use-agent-status";
import { useAgentTurn } from "@/components/agent/use-agent-turn";
import { useStickToEnd } from "@/components/agent/use-stick-to-end";
import { PanelAction } from "@/components/side-panel";
import { agentReducer, initialAgentState } from "@/lib/agent/stream";
import { type ActionResult, refused } from "@/lib/action-result";
import { agentSuggestions } from "@/lib/agent/suggestions";
import { projectAt } from "@/lib/console-nav";
import { AGENT_MODIFIER_KEY, inFront, isChord } from "@/lib/keys";
import { useActiveOrganization, useProjects } from "@/lib/store";
import { cn } from "@/lib/utils";

type View = "conversation" | "history";

const PANEL_ID = "agent-panel";

export function AgentPanel() {
  const { agentOpen, setAgentOpen, searchOpen, setSearchOpen } = useConsoleUi();
  const projects = useProjects();
  const organization = useActiveOrganization()?.slug ?? null;
  // The project the page stands in: none on the organization's own pages or
  // Account's.
  const pageProjectId = projectAt(usePathname(), organization, projects)?.id ?? null;

  const [view, setView] = useState<View>("conversation");
  const [state, dispatch] = useReducer(agentReducer, initialAgentState);
  const streaming = state.turn.phase === "streaming";
  const [history, setHistory] = useState<HistoryState>({ kind: "loading" });
  const [draft, setDraft] = useState("");
  const [notice, setNotice] = useState<string | null>(null);

  // The project the conversation is about. A page of another project starts
  // clean rather than sending its questions into this one's; a page of none —
  // the organization's own, which a citation can lead to — keeps it, so the
  // window can be followed there and asked on. A project no longer listed
  // (moved, deleted, the organization switched) ends it. Reset while
  // rendering, as React has state follow a prop; the turn in flight is
  // aborted as the project moves (`useAgentTurn`).
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

  const agent = useAgentStatus(projectId, agentOpen);
  const { send, cancel: cancelTurn } = useAgentTurn(projectId, dispatch, agent.disabled);
  const { bodyRef, columnRef, onScroll, pin } = useStickToEnd(
    agentOpen && view === "conversation",
  );

  const panelRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  // An answer that arrives after the person has moved on — another project,
  // another conversation opened or started — is dropped. Read through refs,
  // since the answer resolves after the render that asked.
  const shown = useRef({ projectId, conversationId: state.conversationId });
  const historyAsk = useRef(0);
  const openAsk = useRef(0);
  useEffect(() => {
    shown.current = { projectId, conversationId: state.conversationId };
  });

  // ⌘J opens and closes the window. The chord lives with the thing it opens,
  // as ⌘K does in `console-search.tsx`. From the search palette it hands
  // over, the two being the console's ways to ask it something; over any
  // other dialog or menu it does nothing, since the window would open behind
  // it, its field out of reach.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!isChord(e, AGENT_MODIFIER_KEY)) return;
      if (searchOpen) {
        e.preventDefault();
        setSearchOpen(false);
        setAgentOpen(true);
        return;
      }
      if (inFront()) return;
      e.preventDefault();
      setAgentOpen((open) => !open);
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [searchOpen, setSearchOpen, setAgentOpen]);

  // The field is disabled until the agent answers that it is there, so focus
  // opened on the window itself; it moves to the field once there is one.
  useEffect(() => {
    if (agentOpen && agent.status === "enabled" && document.activeElement === panelRef.current) {
      inputRef.current?.focus();
    }
  }, [agentOpen, agent.status]);

  // Closing stops the turn, and the model call behind it.
  useEffect(() => {
    if (!agentOpen && cancelTurn()) dispatch({ type: "stop" });
  }, [agentOpen, cancelTurn]);

  const loadHistory = async (pId: string) => {
    const ask = ++historyAsk.current;
    setHistory({ kind: "loading" });
    const res = await readAgentConversations(pId);
    if (ask !== historyAsk.current || shown.current.projectId !== pId) return;
    setHistory(
      "error" in res
        ? { kind: "error", message: res.error }
        : { kind: "ok", conversations: res.conversations },
    );
  };

  const openConversation = async (conversationId: string) => {
    if (!projectId) return;
    const ask = ++openAsk.current;
    setNotice(null);
    const res = await readAgentConversation(projectId, conversationId);
    if (ask !== openAsk.current || shown.current.projectId !== projectId) return;
    if ("error" in res) {
      setNotice(res.error);
      return;
    }
    cancelTurn();
    pin();
    dispatch({ type: "load", conversation: res.conversation });
    setView("conversation");
    inputRef.current?.focus();
  };

  const deleteConversation = async (conversationId: string) => {
    if (!projectId) return;
    let res: ActionResult;
    try {
      res = await deleteAgentConversationAction(projectId, conversationId);
    } catch (err) {
      // Called from a click, outside any transition, so a failure that is
      // not Next's own would otherwise go unhandled, with the confirm closed
      // and nothing said.
      unstable_rethrow(err);
      res = refused("Couldn't delete that conversation — try again shortly.");
    }
    if (shown.current.projectId !== projectId) return;
    if (!res.ok) {
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
  };

  const newConversation = () => {
    openAsk.current++;
    cancelTurn();
    dispatch({ type: "reset" });
    setView("conversation");
    setNotice(null);
    inputRef.current?.focus();
  };

  const stop = () => {
    cancelTurn();
    dispatch({ type: "stop" });
    inputRef.current?.focus();
  };

  const ask = (raw: string) => {
    const text = raw.trim();
    if (!text || !projectId || streaming) return;
    openAsk.current++;
    setNotice(null);
    setView("conversation");
    pin();
    // Back to the field for the next question: a starter's button goes
    // with the welcome it stood in, and focus would go with it.
    inputRef.current?.focus();
    void send(text, state.conversationId);
  };

  const toggleHistory = () => {
    if (view === "history") {
      setView("conversation");
    } else if (projectId) {
      void loadHistory(projectId);
      setView("history");
    }
  };

  if (!agentOpen) return null;

  const canAsk = Boolean(projectId && agent.status === "enabled");

  const title = (
    <>
      <span className="truncate">Telmoni Agent</span>
      <span
        className={cn(
          "inline-block size-2 shrink-0 rounded-full",
          agent.status === "enabled"
            ? "bg-brand-positive"
            : agent.status === null
              ? "bg-muted-foreground/40"
              : "bg-muted-foreground",
        )}
        aria-hidden
      />
    </>
  );

  const actions = (
    <>
      <PanelAction label="New conversation" disabled={!canAsk} onClick={newConversation}>
        <Plus className="size-4" />
      </PanelAction>
      <PanelAction
        label={view === "history" ? "Back to the conversation" : "Conversation history"}
        disabled={!projectId}
        onClick={toggleHistory}
      >
        {view === "history" ? (
          <MessageSquare className="size-4" />
        ) : (
          <HistoryIcon className="size-4" />
        )}
      </PanelAction>
    </>
  );

  let content: React.ReactNode;
  if (!projectId) {
    content = <Notice>Open a project to ask the agent about it.</Notice>;
  } else if (agent.status === null) {
    content = <Notice>Checking whether the agent is available…</Notice>;
  } else if (agent.status === "disabled") {
    content = (
      <Notice>
        The agent isn&rsquo;t set up on this deployment. Ask whoever runs Telmoni here to
        configure it.
      </Notice>
    );
  } else if (agent.status === "unavailable") {
    content = (
      <Notice
        action={
          <button
            type="button"
            onClick={() => {
              // The button goes as the check begins: focus waits on the
              // window, and moves to the field if the agent answers.
              panelRef.current?.focus();
              agent.retry();
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
  } else if (state.messages.length === 0 && state.turn.phase !== "failed") {
    content = (
      <Welcome
        project={project?.name ?? "this project"}
        suggestions={agentSuggestions(project?.role ?? null)}
        disabled={!canAsk || streaming}
        onAsk={ask}
      />
    );
  } else {
    content = (
      <Conversation messages={state.messages} turn={state.turn} />
    );
  }

  const placeholder = !projectId
    ? "Open a project first"
    : agent.status === null
      ? "One moment…"
      : agent.status !== "enabled"
        ? "The agent is unavailable"
        : state.messages.length > 0
          ? "Reply…"
          : "Ask about this project…";

  return (
    <AgentWindow
      id={PANEL_ID}
      ref={panelRef}
      title={title}
      actions={actions}
      initialFocus={inputRef}
      onClose={() => setAgentOpen(false)}
    >
      <div
        ref={bodyRef}
        onScroll={onScroll}
        className="flex min-h-0 flex-1 flex-col overflow-y-auto px-3.5"
      >
        {/* One column, as wide as reads well: all of a floating window, and
            the middle of the screen at full size. */}
        <div ref={columnRef} className="mx-auto flex w-full max-w-3xl flex-1 flex-col gap-4 pb-3.5">
          {notice && (
            <p role="alert" className="text-sm text-destructive">
              {notice}
            </p>
          )}
          {content}
        </div>
      </div>

      <Composer
        inputRef={inputRef}
        draft={draft}
        onDraft={setDraft}
        placeholder={placeholder}
        about={project ? `Asking about ${project.name}` : "Asks about the project you open"}
        disabled={!canAsk}
        streaming={streaming}
        onSend={() => {
          ask(draft);
          setDraft("");
        }}
        onStop={stop}
      />
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
