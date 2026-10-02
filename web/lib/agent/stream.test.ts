import { describe, expect, it } from "vitest";

import {
  agentEvents,
  agentReducer,
  createSseParser,
  describeRefusal,
  initialAgentState,
  isAgentDisabled,
  parseAgentEvent,
  toolStatus,
  type AgentEvent,
  type AgentState,
} from "./stream";
import { AGENT_TOOLS } from "@/lib/types/agent";

function streamOf(chunks: (string | Uint8Array)[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(typeof c === "string" ? encoder.encode(c) : c);
      controller.close();
    },
  });
}

async function collect(chunks: (string | Uint8Array)[]): Promise<AgentEvent[]> {
  const out: AgentEvent[] = [];
  for await (const e of agentEvents(streamOf(chunks))) out.push(e);
  return out;
}

describe("createSseParser", () => {
  it("dispatches on a blank line and joins multi-line data", () => {
    const seen: [string, string][] = [];
    const p = createSseParser((n, d) => seen.push([n, d]));
    p.push("event: text\ndata: a\ndata: b\n\n");
    expect(seen).toEqual([["text", "a\nb"]]);
  });

  it("holds a CR split from its LF across chunks", () => {
    const seen: [string, string][] = [];
    const p = createSseParser((n, d) => seen.push([n, d]));
    p.push("event: text\r");
    p.push("\ndata: x\r\n\r");
    expect(seen).toEqual([]);
    p.push("\n");
    expect(seen).toEqual([["text", "x"]]);
  });

  it("drops an event the stream never finished", () => {
    const seen: [string, string][] = [];
    const p = createSseParser((n, d) => seen.push([n, d]));
    p.push("event: text\ndata: half");
    p.end();
    expect(seen).toEqual([]);
  });
});

describe("agentEvents", () => {
  it("reads events split mid-line across chunks", async () => {
    const events = await collect([
      'event: te',
      'xt\ndata: {"del',
      'ta":"Good "}\n',
      '\nevent: text\ndata: {"delta":"morning"}\n\n',
    ]);
    expect(events).toEqual([
      { type: "text", delta: "Good " },
      { type: "text", delta: "morning" },
    ]);
  });

  it("reads several events in one chunk, with comments and CRLF", async () => {
    const events = await collect([
      ': keepalive\r\n\r\nevent: tool\r\ndata: {"name":"list_members"}\r\n\r\n' +
        'event: citation\r\ndata: {"index":1,"title":"Members","url":"/p/members"}\r\n\r\n' +
        'event: done\r\ndata: {"conversation_id":"c1","message_id":"m1"}\r\n\r\n',
    ]);
    expect(events).toEqual([
      { type: "tool", name: "list_members" },
      { type: "citation", citation: { index: 1, title: "Members", url: "/p/members" } },
      { type: "done", conversationId: "c1", messageId: "m1" },
    ]);
  });

  it("keeps a citation that has no page, such as an earlier question", async () => {
    const events = await collect([
      'event: citation\ndata: {"index":2,"title":"Earlier question: why","url":null}\n\n',
    ]);
    expect(events).toEqual([
      { type: "citation", citation: { index: 2, title: "Earlier question: why", url: null } },
    ]);
  });

  it("decodes a multi-byte character split across chunks", async () => {
    const bytes = new TextEncoder().encode('event: text\ndata: {"delta":"é"}\n\n');
    const cut = bytes.indexOf(0xc3) + 1;
    const events = await collect([bytes.slice(0, cut), bytes.slice(cut)]);
    expect(events).toEqual([{ type: "text", delta: "é" }]);
  });

  it("ignores unknown events and malformed data", async () => {
    const events = await collect([
      'event: thinking\ndata: {"x":1}\n\n',
      "event: text\ndata: not json\n\n",
      'event: text\ndata: {"delta":5}\n\n',
      'data: {"delta":"unnamed"}\n\n',
      'event: text\ndata: {"delta":"kept"}\n\n',
    ]);
    expect(events).toEqual([{ type: "text", delta: "kept" }]);
  });

  it("reads a mid-stream error", () => {
    expect(
      parseAgentEvent(
        "error",
        '{"conversation_id":null,"type":"/errors/agent/model-unavailable","title":"model unavailable","detail":null}',
      ),
    ).toEqual({
      type: "error",
      conversationId: null,
      problemType: "/errors/agent/model-unavailable",
      title: "model unavailable",
      detail: null,
    });
  });
});

function sent(): AgentState {
  return agentReducer(initialAgentState, {
    type: "send",
    userId: "u1",
    assistantId: "a1",
    message: "Who are the members?",
  });
}

describe("agentReducer", () => {
  it("adds the question and an empty reply, and starts streaming", () => {
    const s = sent();
    expect(s.streaming).toBe(true);
    expect(s.messages.map((m) => [m.role, m.content])).toEqual([
      ["user", "Who are the members?"],
      ["assistant", ""],
    ]);
  });

  it("appends text, shows a tool until text resumes, and collects citations once", () => {
    let s = sent();
    s = agentReducer(s, { type: "event", event: { type: "tool", name: "list_members" } });
    expect(s.tool).toBe("list_members");
    s = agentReducer(s, { type: "event", event: { type: "text", delta: "Two " } });
    expect(s.tool).toBeNull();
    s = agentReducer(s, { type: "event", event: { type: "text", delta: "members [1]." } });
    const citation = { index: 1, title: "Members", url: "/p/members" };
    s = agentReducer(s, { type: "event", event: { type: "citation", citation } });
    s = agentReducer(s, { type: "event", event: { type: "citation", citation } });
    const reply = s.messages[1]!;
    expect(reply.content).toBe("Two members [1].");
    expect(reply.citations).toEqual([citation]);
  });

  it("settles on done with the server's ids", () => {
    let s = sent();
    s = agentReducer(s, { type: "event", event: { type: "text", delta: "Hi" } });
    s = agentReducer(s, {
      type: "event",
      event: { type: "done", conversationId: "c1", messageId: "m1" },
    });
    expect(s.streaming).toBe(false);
    expect(s.conversationId).toBe("c1");
    expect(s.messages[1]!.id).toBe("m1");
  });

  it("drops an empty reply on a mid-stream error and keeps the conversation id", () => {
    let s = sent();
    s = agentReducer(s, {
      type: "event",
      event: {
        type: "error",
        conversationId: "c1",
        problemType: "/errors/agent/model-unavailable",
        title: "model unavailable",
        detail: null,
      },
    });
    expect(s.streaming).toBe(false);
    expect(s.conversationId).toBe("c1");
    expect(s.messages.map((m) => m.role)).toEqual(["user"]);
    expect(s.error).toEqual({ title: "model unavailable", detail: null });
  });

  it("keeps a partial reply when a turn fails", () => {
    let s = sent();
    s = agentReducer(s, { type: "event", event: { type: "text", delta: "Partial" } });
    s = agentReducer(s, { type: "fail", problem: { title: "cut off", detail: null } });
    expect(s.messages[1]!.content).toBe("Partial");
    expect(s.error?.title).toBe("cut off");
  });

  it("ignores events once the turn has settled", () => {
    let s = sent();
    s = agentReducer(s, { type: "fail", problem: { title: "x", detail: null } });
    const after = agentReducer(s, { type: "event", event: { type: "text", delta: "late" } });
    expect(after).toBe(s);
  });

  // A turn cancelled by a reset (another project, a new conversation) can
  // still fail on its way out; that must not land on what replaced it.
  it("ignores a failure when no turn is streaming", () => {
    const reset = agentReducer(sent(), { type: "reset" });
    const after = agentReducer(reset, {
      type: "fail",
      problem: { title: "late", detail: null },
    });
    expect(after).toBe(reset);
  });

  it("loads a conversation and resets to empty", () => {
    const loaded = agentReducer(sent(), {
      type: "load",
      conversation: {
        id: "c9",
        title: "Old",
        messages: [{ id: "m", role: "assistant", content: "Hello", citations: [] }],
      },
    });
    expect(loaded.conversationId).toBe("c9");
    expect(loaded.streaming).toBe(false);
    expect(loaded.messages).toHaveLength(1);
    expect(agentReducer(loaded, { type: "reset" })).toEqual(initialAgentState);
  });
});

describe("describeRefusal", () => {
  it("names the agent's own refusals", () => {
    expect(describeRefusal(503, { type: "/errors/agent/disabled", title: "x", status: 503 }).title)
      .toMatch(/isn't set up/);
    expect(
      describeRefusal(429, {
        type: "/errors/agent/rate-limited",
        title: "x",
        status: 429,
        retry_after_secs: 12,
      }).detail,
    ).toBe("Try again in 12 seconds.");
    expect(
      describeRefusal(502, { type: "/errors/agent/model-unavailable", title: "x", status: 502 })
        .title,
    ).toMatch(/model/);
  });

  it("falls back to the server's title, then to the status", () => {
    expect(describeRefusal(404, { title: "not found", status: 404, detail: "gone" })).toEqual({
      title: "not found",
      detail: "gone",
    });
    expect(describeRefusal(500, "garbage").detail).toMatch(/500/);
    expect(describeRefusal(401, null).title).toMatch(/session/);
  });

  it("words every tool, and one it does not know", () => {
    for (const tool of AGENT_TOOLS) expect(toolStatus(tool)).not.toBe("Looking something up…");
    expect(toolStatus("connector_deliveries")).toBe("Checking connector deliveries…");
    expect(toolStatus("new_tool")).toBe("Looking something up…");
    expect(toolStatus("constructor")).toBe("Looking something up…");
    expect(toolStatus("__proto__")).toBe("Looking something up…");
  });

  it("tells a deployment with no agent from any other refusal", () => {
    expect(isAgentDisabled({ type: "/errors/agent/disabled", title: "x", status: 503 })).toBe(true);
    expect(isAgentDisabled({ type: "/errors/agent/rate-limited", title: "x" })).toBe(false);
    expect(isAgentDisabled("garbage")).toBe(false);
    expect(isAgentDisabled(null)).toBe(false);
  });
});
