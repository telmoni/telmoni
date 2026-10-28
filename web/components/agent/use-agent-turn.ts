import { useCallback, useEffect, useRef, type Dispatch } from "react";

import {
  agentEvents,
  describeRefusal,
  isAgentDisabled,
  type AgentAction,
} from "@/lib/agent/stream";

/** The turn streaming now: its send, and its cancel. ⚠ **Every way out of a
 *  turn aborts it**: a stop, a new conversation, another one opened, another
 *  project, the window closed. Left running, its events landed in the next
 *  turn's reply, and its `done` handed the next question to the old
 *  conversation. `onDisabled` hears a turn refused because the agent is off. */
export function useAgentTurn(
  projectId: string | null,
  dispatch: Dispatch<AgentAction>,
  onDisabled: () => void,
) {
  const turnRef = useRef<AbortController | null>(null);

  /** Aborts the turn in flight; whether there was one. */
  const cancel = useCallback(() => {
    const turn = turnRef.current;
    turnRef.current = null;
    turn?.abort();
    return turn !== null;
  }, []);
  useEffect(
    () => () => {
      cancel();
    },
    [projectId, cancel],
  );

  const send = async (text: string, conversationId: string | null) => {
    if (!projectId) return;
    dispatch({
      type: "send",
      userId: crypto.randomUUID(),
      assistantId: crypto.randomUUID(),
      message: text,
    });

    cancel();
    const turn = new AbortController();
    turnRef.current = turn;
    // Only the turn the window is on may change it: one that was cancelled
    // (a stop, a reset, another project) can still have a dispatch in flight.
    const current = () => turnRef.current === turn;
    try {
      const response = await fetch("/api/v1/agent/turns", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          projectId,
          conversationId: conversationId ?? undefined,
          message: text,
        }),
        signal: turn.signal,
      });

      if (!response.ok) {
        const body: unknown = await response.json().catch(() => null);
        if (!current()) return;
        if (isAgentDisabled(body)) onDisabled();
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
  };

  return { send, cancel };
}
