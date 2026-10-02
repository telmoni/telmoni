import { z } from "zod";

import {
  AGENT_PROBLEM,
  type AgentCitation,
  type AgentConversation,
  type AgentMessage,
  type AgentTool,
} from "@/lib/types/agent";

export type AgentEvent =
  | { type: "text"; delta: string }
  | { type: "tool"; name: string }
  | { type: "citation"; citation: AgentCitation }
  | { type: "done"; conversationId: string; messageId: string }
  | {
      type: "error";
      conversationId: string | null;
      problemType: string;
      title: string;
      detail: string | null;
    };

const TextData = z.object({ delta: z.string() });
const ToolData = z.object({ name: z.string() });
// A citation as the server sends it, streamed and stored alike.
export const CitationData = z.object({
  index: z.number().int().nonnegative(),
  title: z.string(),
  url: z.string().nullable(),
}) satisfies z.ZodType<AgentCitation>;
const DoneData = z.object({ conversation_id: z.string(), message_id: z.string() });
const ErrorData = z.object({
  conversation_id: z.string().nullable(),
  type: z.string(),
  title: z.string(),
  detail: z.string().nullable(),
});

// `null` for an event this console does not know, or one whose data is not
// the shape it expects: a server a release ahead may add events, and a stream
// that dropped a turn over one it could not read would lose the reply.
export function parseAgentEvent(name: string, data: string): AgentEvent | null {
  let body: unknown;
  try {
    body = JSON.parse(data);
  } catch {
    return null;
  }
  switch (name) {
    case "text": {
      const p = TextData.safeParse(body);
      return p.success ? { type: "text", delta: p.data.delta } : null;
    }
    case "tool": {
      const p = ToolData.safeParse(body);
      return p.success ? { type: "tool", name: p.data.name } : null;
    }
    case "citation": {
      const p = CitationData.safeParse(body);
      return p.success ? { type: "citation", citation: p.data } : null;
    }
    case "done": {
      const p = DoneData.safeParse(body);
      return p.success
        ? {
            type: "done",
            conversationId: p.data.conversation_id,
            messageId: p.data.message_id,
          }
        : null;
    }
    case "error": {
      const p = ErrorData.safeParse(body);
      return p.success
        ? {
            type: "error",
            conversationId: p.data.conversation_id,
            problemType: p.data.type,
            title: p.data.title,
            detail: p.data.detail,
          }
        : null;
    }
    default:
      return null;
  }
}

/**
 * The event-stream framing, fed text as it arrives.
 *
 * ⚠ **A chunk boundary falls wherever the network puts it**, mid-line and
 * between the `\r` and `\n` of one terminator included. A trailing `\r` is
 * therefore held back until the next chunk says whether a `\n` follows it;
 * reading it as a line end on its own would split one CRLF into a line and
 * an empty line, and an empty line dispatches the event half-read.
 */
export function createSseParser(onEvent: (name: string, data: string) => void) {
  let buffer = "";
  let name = "";
  let data: string[] = [];

  const line = (text: string) => {
    if (text === "") {
      if (data.length > 0) onEvent(name || "message", data.join("\n"));
      name = "";
      data = [];
      return;
    }
    if (text.startsWith(":")) return;
    const colon = text.indexOf(":");
    const field = colon === -1 ? text : text.slice(0, colon);
    let value = colon === -1 ? "" : text.slice(colon + 1);
    if (value.startsWith(" ")) value = value.slice(1);
    if (field === "event") name = value;
    else if (field === "data") data.push(value);
  };

  const drain = (final: boolean) => {
    for (;;) {
      const at = buffer.search(/[\r\n]/);
      if (at === -1) break;
      if (buffer[at] === "\r" && at === buffer.length - 1 && !final) break;
      const width = buffer[at] === "\r" && buffer[at + 1] === "\n" ? 2 : 1;
      line(buffer.slice(0, at));
      buffer = buffer.slice(at + width);
    }
  };

  return {
    push(text: string) {
      buffer += text;
      drain(false);
    },
    // An event with no blank line after it is one the stream never finished
    // sending, so it is dropped rather than dispatched.
    end() {
      drain(true);
      buffer = "";
      name = "";
      data = [];
    },
  };
}

export async function* agentEvents(
  body: ReadableStream<Uint8Array>,
): AsyncGenerator<AgentEvent> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  const ready: AgentEvent[] = [];
  const parser = createSseParser((name, data) => {
    const event = parseAgentEvent(name, data);
    if (event) ready.push(event);
  });
  let finished = false;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) {
        finished = true;
        parser.push(decoder.decode());
        parser.end();
      } else {
        parser.push(decoder.decode(value, { stream: true }));
      }
      while (ready.length > 0) yield ready.shift()!;
      if (done) return;
    }
  } finally {
    // A caller that stops reading early cancels the body, so the connection
    // behind it closes rather than buffering a reply nobody reads.
    if (!finished) await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

export interface AgentProblem {
  title: string;
  detail: string | null;
}

export interface AgentState {
  conversationId: string | null;
  messages: AgentMessage[];
  streaming: boolean;
  tool: string | null;
  error: AgentProblem | null;
}

export const initialAgentState: AgentState = {
  conversationId: null,
  messages: [],
  streaming: false,
  tool: null,
  error: null,
};

export type AgentAction =
  | { type: "send"; userId: string; assistantId: string; message: string }
  | { type: "event"; event: AgentEvent }
  | { type: "fail"; problem: AgentProblem }
  | { type: "load"; conversation: AgentConversation }
  | { type: "reset" };

function withLastAssistant(
  messages: AgentMessage[],
  change: (m: AgentMessage) => AgentMessage,
): AgentMessage[] {
  const last = messages[messages.length - 1];
  if (!last || last.role !== "assistant") return messages;
  return [...messages.slice(0, -1), change(last)];
}

// A reply that failed before a word of it arrived leaves no empty bubble
// behind; one that failed partway keeps what it said, under the error.
function dropEmptyReply(messages: AgentMessage[]): AgentMessage[] {
  const last = messages[messages.length - 1];
  if (last && last.role === "assistant" && last.content === "") {
    return messages.slice(0, -1);
  }
  return messages;
}

export function agentReducer(state: AgentState, action: AgentAction): AgentState {
  switch (action.type) {
    case "send":
      return {
        ...state,
        messages: [
          ...state.messages,
          { id: action.userId, role: "user", content: action.message, citations: [] },
          { id: action.assistantId, role: "assistant", content: "", citations: [] },
        ],
        streaming: true,
        tool: null,
        error: null,
      };
    case "fail":
      // Only a turn in progress can fail. One already reset away (another
      // project, a new conversation) can still fail on its way out, and that
      // must not land on what replaced it.
      if (!state.streaming) return state;
      return {
        ...state,
        messages: dropEmptyReply(state.messages),
        streaming: false,
        tool: null,
        error: action.problem,
      };
    case "load":
      return {
        ...initialAgentState,
        conversationId: action.conversation.id,
        messages: action.conversation.messages,
      };
    case "reset":
      return initialAgentState;
    case "event":
      return applyEvent(state, action.event);
  }
}

function applyEvent(state: AgentState, event: AgentEvent): AgentState {
  if (!state.streaming) return state;
  switch (event.type) {
    case "text":
      return {
        ...state,
        tool: null,
        messages: withLastAssistant(state.messages, (m) => ({
          ...m,
          content: m.content + event.delta,
        })),
      };
    case "tool":
      return { ...state, tool: event.name };
    case "citation":
      return {
        ...state,
        messages: withLastAssistant(state.messages, (m) =>
          m.citations.some((c) => c.index === event.citation.index)
            ? m
            : { ...m, citations: [...m.citations, event.citation] },
        ),
      };
    case "done":
      return {
        ...state,
        conversationId: event.conversationId,
        streaming: false,
        tool: null,
        messages: withLastAssistant(state.messages, (m) => ({
          ...m,
          id: event.messageId,
        })),
      };
    case "error":
      return {
        ...state,
        conversationId: event.conversationId ?? state.conversationId,
        messages: dropEmptyReply(state.messages),
        streaming: false,
        tool: null,
        error: { title: event.title, detail: event.detail },
      };
  }
}

const TOOL_STATUS: Record<AgentTool, string> = {
  search: "Searching this project…",
  list_members: "Checking members…",
  list_connectors: "Checking connectors…",
  connector_deliveries: "Checking connector deliveries…",
  audit_events: "Reading the audit log…",
};

// The name is whatever the model called, so it is looked up as an own key: a
// call to `constructor` must not find `Object`'s.
export function toolStatus(name: string): string {
  return Object.hasOwn(TOOL_STATUS, name)
    ? TOOL_STATUS[name as AgentTool]
    : "Looking something up…";
}

const RefusalBody = z.object({
  type: z.string().optional(),
  title: z.string().optional(),
  detail: z.string().nullable().optional(),
  retry_after_secs: z.number().optional(),
});

// Whether a refusal says the deployment has no agent, which the panel keeps
// as its status rather than as one turn's error.
export function isAgentDisabled(body: unknown): boolean {
  const parsed = RefusalBody.safeParse(body);
  return parsed.success && parsed.data.type === AGENT_PROBLEM.disabled;
}

// What the panel says when a turn is refused before it streams. The agent's
// own three refusals get a sentence of their own; anything else says what the
// server said, or the status when it said nothing readable.
export function describeRefusal(
  status: number,
  body: unknown,
  retryAfterHeader: string | null = null,
): AgentProblem {
  const parsed = RefusalBody.safeParse(body);
  const problem = parsed.success ? parsed.data : {};
  switch (problem.type) {
    case AGENT_PROBLEM.disabled:
      return {
        title: "The agent isn't set up on this deployment.",
        detail: "Ask whoever runs Telmoni here to configure it.",
      };
    case AGENT_PROBLEM.rateLimited: {
      const secs = problem.retry_after_secs ?? Number(retryAfterHeader ?? NaN);
      return {
        title: "You've sent a lot of messages in a short time.",
        detail: Number.isFinite(secs) && secs > 0
          ? `Try again in ${Math.ceil(secs)} second${Math.ceil(secs) === 1 ? "" : "s"}.`
          : "Try again in a moment.",
      };
    }
    case AGENT_PROBLEM.modelUnavailable:
      return {
        title: "The model isn't answering right now.",
        detail: "Try again shortly.",
      };
  }
  if (status === 401) {
    return { title: "Your session expired.", detail: "Sign in again to keep going." };
  }
  if (status === 429) {
    return { title: "Too many requests.", detail: "Slow down a moment, then try again." };
  }
  if (problem.title) {
    return { title: problem.title, detail: problem.detail ?? null };
  }
  return {
    title: "The agent couldn't answer.",
    detail: `The request failed (${status}). Try again shortly.`,
  };
}
