"use client";

import { Reply, Sources } from "@/components/agent/agent-reply";
import { AgentThinking } from "@/components/agent/agent-thinking";
import { toolStatus, type AgentProblem } from "@/lib/agent/stream";
import type { AgentMessage } from "@/lib/types/agent";

export function Conversation({
  messages,
  streaming,
  tool,
  error,
  stopped,
}: {
  messages: AgentMessage[];
  streaming: boolean;
  tool: string | null;
  error: AgentProblem | null;
  stopped: boolean;
}) {
  const partial = stopped && messages[messages.length - 1]?.role === "assistant";
  return (
    <div role="log" aria-live="polite" aria-busy={streaming} className="grid gap-4">
      {messages.map((m, i) =>
        m.role === "user" ? (
          <p
            key={m.id}
            className="ml-8 justify-self-end rounded-lg bg-muted px-3 py-2 text-sm whitespace-pre-wrap wrap-break-word"
          >
            {m.content}
          </p>
        ) : (
          <div key={m.id} className="grid gap-3">
            {m.content !== "" && <Reply content={m.content} citations={m.citations} />}
            {m.content === "" && streaming && i === messages.length - 1 && !tool && (
              <AgentThinking />
            )}
            {m.citations.length > 0 && <Sources citations={m.citations} />}
          </div>
        ),
      )}
      {streaming && tool && (
        <p role="status" className="text-xs text-muted-foreground">
          {toolStatus(tool)}
        </p>
      )}
      {/* The server keeps the question and drops a reply it never finished,
          so a stopped one is gone once the conversation is opened again. */}
      {stopped && (
        <p className="text-xs text-muted-foreground">
          {partial ? "Stopped. This reply isn’t saved." : "Stopped."}
        </p>
      )}
      {error && (
        <div role="alert" className="grid gap-0.5 text-sm">
          <p className="font-medium text-destructive">{error.title}</p>
          {error.detail && <p className="text-muted-foreground">{error.detail}</p>}
        </div>
      )}
    </div>
  );
}
