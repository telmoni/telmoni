"use client";

import { createContext, useContext, useState, type ReactNode } from "react";

import { Reply } from "@/components/agent/agent-reply";
import { Button } from "@/components/ui/button";
import {
  FIELD_LABEL,
  cutFrom,
  drawnAs,
  failureOf,
  keptFileOf,
  instructionsOf,
  messagesOf,
  metadataOf,
  readable,
  readableValue,
  scrubbedFrom,
  type Message,
  type Part,
} from "@/lib/messages";
import { ContentField } from "@/lib/types/enums";

// What a span kept of what a person or a model wrote, in full, as the
// conventions carry it: messages under their roles, their text as Markdown,
// tool calls and their results as indented JSON, reasoning folded. Each
// opening was filed in the audit log before the server sent it.

const ROLE_LABEL: Readonly<Record<string, string>> = {
  system: "System",
  user: "User",
  assistant: "Assistant",
  tool: "Tool",
};

/** A role as the trace names it: the conventions' four by name, any other as sent. */
function roleLabel(role: string): string {
  return ROLE_LABEL[role] ?? role;
}

/** Text a person reads in a block, wrapped and scrolled within its width. */
function Pre({ children }: { children: string }) {
  return (
    <pre className="overflow-x-auto rounded-md bg-muted p-gutter text-xs break-all whitespace-pre-wrap">
      {children}
    </pre>
  );
}

/** The project whose kept files a conversation draws: none where nobody
 *  provided it, and a file is then named by its type. */
const MediaProject = createContext<string | null>(null);

/** Draws the kept files of the conversations under it from `projectId`. */
export function MediaProjectProvider({
  projectId,
  children,
}: {
  projectId: string;
  children: ReactNode;
}) {
  return <MediaProject.Provider value={projectId}>{children}</MediaProject.Provider>;
}

/** A part's kind and type, said in a line. */
function MediaLabel({ part, note }: { part: Extract<Part, { type: "media" }>; note: string }) {
  return (
    <p className="text-xs text-muted-foreground">
      {part.modality ?? "A file"}
      {part.mimeType ? ` (${part.mimeType})` : ""}, {note}
    </p>
  );
}

/** An image, audio or a file: one this deployment keeps drawn from it, read
 *  as its span's content is; a link drawn only when the reader asks, since
 *  drawing it has their browser call a host the agent chose; a provider's
 *  file named by its id. */
function MediaPart({ part }: { part: Extract<Part, { type: "media" }> }) {
  const projectId = useContext(MediaProject);
  const [failed, setFailed] = useState(false);
  const [shown, setShown] = useState(false);
  const hash = keptFileOf(part);
  if (hash && projectId && !failed) {
    const href = `/api/v1/media/${hash}?project=${encodeURIComponent(projectId)}`;
    switch (drawnAs(part.mimeType)) {
      case "image":
        return (
          // A kept file, read through its own lane, which a page's optimizer never fetches.
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={href}
            alt={part.modality ?? "An image"}
            className="max-h-80 max-w-full rounded-md border border-border"
            onError={() => setFailed(true)}
          />
        );
      case "audio":
        return <audio controls src={href} className="w-full" onError={() => setFailed(true)} />;
      case "download":
        return (
          <a href={href} download className="text-xs underline underline-offset-4">
            Download {part.mimeType ?? "the file"}
          </a>
        );
    }
  }
  if (hash) return <MediaLabel part={part} note={failed ? "not kept" : "kept, not drawn here"} />;
  if (part.kind === "uri" && part.uri) {
    return shown ? (
      <a href={part.uri} target="_blank" rel="noopener noreferrer" className="text-xs break-all underline underline-offset-4">
        {part.uri}
      </a>
    ) : (
      <div className="flex items-center gap-2">
        <MediaLabel part={part} note="at a link the agent named" />
        <Button variant="outline" size="sm" onClick={() => setShown(true)}>
          Show link
        </Button>
      </div>
    );
  }
  if (part.kind === "file" && part.fileId) return <MediaLabel part={part} note={`a provider's file, ${part.fileId}`} />;
  return <MediaLabel part={part} note="not kept" />;
}

/** One part of a message. */
function PartView({ part }: { part: Part }) {
  switch (part.type) {
    case "text":
      return <Reply content={part.content} citations={[]} />;
    case "reasoning":
      return (
        <details className="text-muted-foreground">
          <summary className="cursor-pointer text-xs">Reasoning</summary>
          <div className="pt-1">
            <Reply content={part.content} citations={[]} />
          </div>
        </details>
      );
    case "tool_call":
      return (
        <div className="grid gap-1">
          <p className="text-xs text-muted-foreground">
            Called <span className="font-medium text-foreground">{part.name}</span>
            {part.id ? ` (${part.id})` : ""}
          </p>
          {part.arguments !== undefined && <Pre>{readableValue(part.arguments)}</Pre>}
        </div>
      );
    case "tool_call_response":
      return (
        <div className="grid gap-1">
          <p className="text-xs text-muted-foreground">Result{part.id ? ` of ${part.id}` : ""}</p>
          <Pre>{readableValue(part.response)}</Pre>
        </div>
      );
    case "media":
      return <MediaPart part={part} />;
    case "other":
      return <Pre>{JSON.stringify(part.value, null, 2)}</Pre>;
  }
}

/** Parts in order, a message's or the system instructions'. */
function Parts({ parts }: { parts: readonly Part[] }) {
  return (
    <div className="grid gap-2">
      {parts.map((part, i) => (
        <PartView key={i} part={part} />
      ))}
    </div>
  );
}

/** Messages in order, each under its role. */
export function Conversation({ messages }: { messages: readonly Message[] }) {
  return (
    <ol className="grid gap-gutter">
      {messages.map((message, i) => (
        <li key={i} className="grid gap-1">
          <p className="text-xs font-medium text-muted-foreground">
            {roleLabel(message.role)}
            {message.name ? ` · ${message.name}` : ""}
            {message.finishReason ? (
              <span className="font-normal"> · stopped: {message.finishReason}</span>
            ) : null}
          </p>
          <Parts parts={message.parts} />
        </li>
      ))}
    </ol>
  );
}

/** One field's value: messages as a conversation, instructions as parts, the rest indented. */
function FieldValue({ field, value }: { field: ContentField; value: string }) {
  if (field === ContentField.InputMessages || field === ContentField.OutputMessages) {
    const messages = messagesOf(value);
    if (messages) return <Conversation messages={messages} />;
  }
  if (field === ContentField.SystemInstructions) {
    const parts = instructionsOf(value);
    if (parts) return <Parts parts={parts} />;
  }
  return <Pre>{readable(value)}</Pre>;
}

/** A labelled block of a span's content. */
function Section({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section className="grid gap-1.5">
      <h4 className="text-xs font-medium uppercase tracking-wide text-muted-foreground">{label}</h4>
      {children}
    </section>
  );
}

/** Which fields an SDK cut to fit an export, said where any was. */
function CutNote({ fields }: { fields: Readonly<Record<string, string>> }) {
  const cut = cutFrom(fields);
  if (cut.length === 0) return null;
  return (
    <p className="text-xs text-muted-foreground">
      Cut to fit an export, and not kept: {cut.join(", ")}.
    </p>
  );
}

/** What the agent's scrubbers replaced before the span left it, and the
 *  fields they failed on and dropped, said where they did either. */
export function ScrubbedNote({ attributes }: { attributes: Readonly<Record<string, string>> }) {
  const scrubbed = scrubbedFrom(attributes);
  if (!scrubbed) return null;
  return (
    <div className="grid gap-1 text-xs text-muted-foreground">
      {scrubbed.replaced.length > 0 && (
        <p>Scrubbed by the SDK or at the door: {scrubbed.replaced.join(", ")}.</p>
      )}
      {scrubbed.dropped.length > 0 && (
        <p>Dropped, its scrubber failing on it: {scrubbed.dropped.join(", ")}.</p>
      )}
    </div>
  );
}

/** A failure as the span kept it: its class and message, its stack trace
 *  folded. */
function Failure({ fields }: { fields: Readonly<Record<string, string>> }) {
  const failure = failureOf(fields);
  if (!failure) return null;
  return (
    <Section label="Failure">
      <div className="grid gap-1.5">
        <p className="break-words text-destructive">
          {failure.type && <span className="font-medium">{failure.type}: </span>}
          {failure.message}
        </p>
        {failure.stacktrace && (
          <details>
            <summary className="cursor-pointer text-xs text-muted-foreground">Stack trace</summary>
            <div className="pt-1">
              <Pre>{failure.stacktrace}</Pre>
            </div>
          </details>
        )}
      </div>
    </Section>
  );
}

/** Everything a span kept, in the order a call reads: its failure first, what
 *  it was told and offered, what it was sent and answered, then the rest, its
 *  metadata last. */
export function SpanContent({ fields }: { fields: Readonly<Record<string, string>> }) {
  const metadata = metadataOf(fields);
  return (
    <div className="grid gap-4 text-sm">
      <CutNote fields={fields} />
      <Failure fields={fields} />
      {FIELD_LABEL.map(([field, label]) => {
        const value = fields[field];
        return value === undefined ? null : (
          <Section key={field} label={label}>
            <FieldValue field={field} value={value} />
          </Section>
        );
      })}
      {metadata.length > 0 && (
        <Section label="Metadata">
          <dl className="grid gap-2">
            {metadata.map(([name, value]) => (
              <div key={name} className="grid gap-1">
                <dt className="text-xs text-muted-foreground">{name}</dt>
                <dd>
                  <Pre>{readable(value)}</Pre>
                </dd>
              </div>
            ))}
          </dl>
        </Section>
      )}
    </div>
  );
}

/** A trace's turn: the person's message, then the agent's answer, from the
 *  agent's own span; a line where it kept none, or where none could be read. */
export function TraceTurn({ fields }: { fields: Readonly<Record<string, string>> | null | undefined }) {
  if (fields === null) {
    return <p className="text-xs text-muted-foreground">This trace&apos;s content could not be read. Reload to try again.</p>;
  }
  const input = fields?.[ContentField.InputMessages];
  const output = fields?.[ContentField.OutputMessages];
  if (input === undefined && output === undefined) {
    return <p className="text-xs text-muted-foreground">This trace kept no content.</p>;
  }
  return (
    <div className="grid gap-gutter text-sm">
      {fields && <CutNote fields={fields} />}
      {input !== undefined && <FieldValue field={ContentField.InputMessages} value={input} />}
      {output !== undefined && <FieldValue field={ContentField.OutputMessages} value={output} />}
    </div>
  );
}
