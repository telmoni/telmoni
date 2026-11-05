import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  cutFrom,
  drawnAs,
  failureOf,
  keptFileOf,
  fieldsOf,
  instructionsOf,
  messagesOf,
  metadataOf,
  readable,
  readableValue,
  scrubbedFrom,
} from "./messages";

describe("messagesOf", () => {
  it("reads each message's role, parts and why a model stopped", () => {
    const source = JSON.stringify([
      { role: "user", parts: [{ type: "text", content: "Weather in Paris?" }] },
      {
        role: "assistant",
        parts: [
          { type: "reasoning", content: "Look it up." },
          { type: "tool_call", id: "c1", name: "weather", arguments: { city: "Paris" } },
        ],
        finish_reason: "tool_call",
      },
      { role: "tool", parts: [{ type: "tool_call_response", id: "c1", response: "Rain" }] },
      { role: "user", parts: [{ type: "blob", modality: "image", mime_type: "image/png", content: "aGk=" }] },
    ]);
    expect(messagesOf(source)).toEqual([
      { role: "user", parts: [{ type: "text", content: "Weather in Paris?" }] },
      {
        role: "assistant",
        parts: [
          { type: "reasoning", content: "Look it up." },
          { type: "tool_call", id: "c1", name: "weather", arguments: { city: "Paris" } },
        ],
        finishReason: "tool_call",
      },
      { role: "tool", parts: [{ type: "tool_call_response", id: "c1", response: "Rain" }] },
      { role: "user", parts: [{ type: "media", kind: "blob", modality: "image", mimeType: "image/png" }] },
    ]);
  });

  it("keeps a part of a type it does not know as it arrived", () => {
    const part = { type: "server_tool_call", name: "search" };
    expect(messagesOf(JSON.stringify([{ role: "assistant", parts: [part] }]))).toEqual([
      { role: "assistant", parts: [{ type: "other", value: part }] },
    ]);
  });

  it("answers null for anything that is not the conventions' shape", () => {
    for (const source of ["hello", "{}", '[{"role":"user"}]', '[{"role":"user","parts":["hi"]}]']) {
      expect(messagesOf(source), source).toBeNull();
    }
  });
});

describe("instructionsOf", () => {
  it("reads system instructions as parts, or null", () => {
    expect(instructionsOf('[{"type":"text","content":"Be brief."}]')).toEqual([
      { type: "text", content: "Be brief." },
    ]);
    expect(instructionsOf("Be brief.")).toBeNull();
  });
});

describe("readable", () => {
  it("indents JSON and leaves other text as it arrived", () => {
    expect(readable('{"a":1}')).toBe('{\n  "a": 1\n}');
    expect(readable("plain text")).toBe("plain text");
    expect(readableValue({ city: "Paris" })).toBe('{\n  "city": "Paris"\n}');
    expect(readableValue('{"city":"Paris"}')).toBe('{\n  "city": "Paris"\n}');
  });
});

describe("cutFrom and metadataOf", () => {
  it("names the fields an SDK cut, and the fields no label names", () => {
    const fields = {
      "telmoni.content_cut": '["gen_ai.input.messages"]',
      "gen_ai.output.messages": "[]",
      "app.ticket": '{"id":42}',
      "app.customer": "acme",
    };
    expect(cutFrom(fields)).toEqual(["gen_ai.input.messages"]);
    expect(cutFrom({})).toEqual([]);
    expect(metadataOf(fields)).toEqual([
      ["app.customer", "acme"],
      ["app.ticket", '{"id":42}'],
    ]);
  });
});

describe("scrubbedFrom", () => {
  it("names each kind replaced with its count, and the fields dropped", () => {
    expect(
      scrubbedFrom({
        "telmoni.scrubbed": '{"dropped:gen_ai.input.messages":1,"pii:email":2,"pii:ip":1,"secret:jwt":3}',
      }),
    ).toEqual({
      replaced: ["2 email addresses", "1 IP address", "3 JWTs"],
      dropped: ["gen_ai.input.messages"],
    });
  });

  it("says nothing of a span that carries no count, or one it cannot read", () => {
    expect(scrubbedFrom({})).toBeNull();
    expect(scrubbedFrom({ "telmoni.scrubbed": "not json" })).toBeNull();
    expect(scrubbedFrom({ "telmoni.scrubbed": '{"pii:email":-1}' })).toBeNull();
  });

  it("names a kind it does not know by its key", () => {
    expect(scrubbedFrom({ "telmoni.scrubbed": '{"secret:new_kind":1}' })?.replaced).toEqual(["1 secret:new_kind"]);
  });
});

describe("fieldsOf", () => {
  it("tells a span that kept fields from one that kept none and content unread", () => {
    const content = [{ run_id: "r", span_id: "s1", fields: { "gen_ai.input.messages": "[]" } }];
    expect(fieldsOf(content, "s1")).toEqual({ "gen_ai.input.messages": "[]" });
    expect(fieldsOf(content, "s2")).toBeUndefined();
    expect(fieldsOf(null, "s1")).toBeNull();
  });
});

describe("failureOf", () => {
  it("reads a failure's class, the exception's message before the status's, and its stack trace", () => {
    expect(
      failureOf({
        "exception.type": "ValueError",
        "exception.message": "boom",
        "exception.stacktrace": "Traceback",
        "otel.status_description": "status boom",
      }),
    ).toEqual({ type: "ValueError", message: "boom", stacktrace: "Traceback" });
    expect(failureOf({ "otel.status_description": "status boom" })).toEqual({
      message: "status boom",
    });
    expect(failureOf({ "gen_ai.input.messages": "[]" })).toBeNull();
  });

  it("keeps a failure's fields out of the metadata", () => {
    expect(
      metadataOf({ "exception.message": "boom", "otel.status_description": "x", "app.k": "v" }),
    ).toEqual([["app.k", "v"]]);
  });
});

describe("kept files", () => {
  it("reads a file part's id and a uri part's link, and names a file kept here by its hash", () => {
    const hash = "b".repeat(64);
    const [message] = messagesOf(
      JSON.stringify([
        {
          role: "user",
          parts: [
            { type: "file", modality: "image", mime_type: "image/png", file_id: `telmoni:${hash}` },
            { type: "uri", modality: "image", uri: "https://example.com/cat.png" },
            { type: "file", file_id: "file-abc" },
          ],
        },
      ]),
    )!;
    const [kept, linked, provider] = message!.parts;
    expect(keptFileOf(kept!)).toBe(hash);
    expect(linked).toEqual({ type: "media", kind: "uri", modality: "image", uri: "https://example.com/cat.png" });
    expect(keptFileOf(provider!)).toBeNull();
    expect(keptFileOf({ type: "media", kind: "file", fileId: "telmoni:short" })).toBeNull();
  });

  // What the SDKs send names each file they put (`sdk/fixtures/media.json`),
  // so a drift from their prefix would draw none of them. Each `file` part is
  // read alone, since the fixture's fields hold parts no message parses.
  it("names every file the SDKs put by the hash they put it under", () => {
    const fixture = JSON.parse(
      readFileSync(fileURLToPath(new URL("../../sdk/fixtures/media.json", import.meta.url)), "utf8"),
    ) as { uploads: { path: string }[]; sent: Record<string, unknown> };
    const files: unknown[] = [];
    const walk = (value: unknown): void => {
      if (Array.isArray(value)) return value.forEach(walk);
      if (value === null || typeof value !== "object") return;
      if ((value as { type?: unknown }).type === "file") files.push(value);
      Object.values(value).forEach(walk);
    };
    walk(fixture.sent);
    const named = new Set(
      files
        .map((part) => messagesOf(JSON.stringify([{ role: "user", parts: [part] }]))![0]!.parts[0]!)
        .map(keptFileOf)
        .filter((hash) => hash !== null),
    );
    const put = new Set(fixture.uploads.map((upload) => upload.path.split("/").at(-1)));
    expect(named).toEqual(put);
  });

  it("draws the four image types inline, plays audio, and offers anything else as a download", () => {
    for (const type of ["image/png", "image/jpeg", "image/gif", "image/webp"]) {
      expect(drawnAs(type)).toBe("image");
    }
    expect(drawnAs("audio/wav")).toBe("audio");
    expect(drawnAs("image/svg+xml")).toBe("download");
    expect(drawnAs(undefined)).toBe("download");
  });
});
