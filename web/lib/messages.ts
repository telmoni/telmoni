import { z } from "zod";

import { Content, ContentField } from "@/lib/types/enums";
import type { SpanFields } from "@/lib/types/traces";

// What a span's content fields hold, read as the GenAI conventions write
// them: messages, each under its role, made of parts. A field that is not
// that shape is shown as it arrived, never guessed at.

/** One part of a message, as the trace draws it. */
export type Part =
  | { type: "text"; content: string }
  | { type: "reasoning"; content: string }
  | { type: "tool_call"; id?: string; name: string; arguments?: unknown }
  | { type: "tool_call_response"; id?: string; response: unknown }
  /** An image, audio or a file: inline (`blob`), at a link (`uri`), or by an
   *  id (`file`), a file this deployment keeps named `telmoni:` and its hash. */
  | {
      type: "media";
      kind: "blob" | "uri" | "file";
      modality?: string;
      mimeType?: string;
      uri?: string;
      fileId?: string;
    }
  /** A part of a type the conventions add later, shown as it arrived. */
  | { type: "other"; value: unknown };

/** One message: who wrote it, what it says, and why a model stopped. */
export type Message = { role: string; name?: string; parts: Part[]; finishReason?: string };

const text = z.object({ type: z.literal("text"), content: z.string() });
const reasoning = z.object({ type: z.literal("reasoning"), content: z.string() });
const toolCall = z.object({
  type: z.literal("tool_call"),
  id: z.string().optional(),
  name: z.string(),
  arguments: z.unknown().optional(),
});
const toolCallResponse = z.object({
  type: z.literal("tool_call_response"),
  id: z.string().optional(),
  response: z.unknown(),
});
const media = z.object({
  type: z.enum(["blob", "uri", "file"]),
  modality: z.string().optional(),
  mime_type: z.string().optional(),
  uri: z.string().optional(),
  file_id: z.string().optional(),
});
const anyPart = z.object({ type: z.string() }).loose();

/** A part as the conventions spell it, or `other` for a type they add later. */
function partOf(raw: unknown): Part | null {
  for (const parsed of [text, reasoning].map((schema) => schema.safeParse(raw))) {
    if (parsed.success) return parsed.data;
  }
  const call = toolCall.safeParse(raw);
  if (call.success) return call.data;
  const answer = toolCallResponse.safeParse(raw);
  if (answer.success) return answer.data;
  const file = media.safeParse(raw);
  if (file.success) {
    return {
      type: "media",
      kind: file.data.type,
      ...(file.data.modality ? { modality: file.data.modality } : {}),
      ...(file.data.mime_type ? { mimeType: file.data.mime_type } : {}),
      ...(file.data.uri ? { uri: file.data.uri } : {}),
      ...(file.data.file_id ? { fileId: file.data.file_id } : {}),
    };
  }
  return anyPart.safeParse(raw).success ? { type: "other", value: raw } : null;
}

const messageShape = z.object({
  role: z.string(),
  name: z.string().optional(),
  parts: z.array(z.unknown()),
  finish_reason: z.string().optional(),
});

/** The text as JSON, or `undefined` where it is not. */
function json(source: string): unknown {
  try {
    return JSON.parse(source);
  } catch {
    return undefined;
  }
}

/** Parts in order, or `null` where any is not a part. */
function partsIn(raw: unknown[]): Part[] | null {
  const parts = raw.map(partOf);
  return parts.every((part) => part !== null) ? (parts as Part[]) : null;
}

/** A messages field as messages, or `null` where it is not the conventions' shape. */
export function messagesOf(source: string): Message[] | null {
  const parsed = z.array(messageShape).safeParse(json(source));
  if (!parsed.success) return null;
  const messages: Message[] = [];
  for (const message of parsed.data) {
    const parts = partsIn(message.parts);
    if (parts === null) return null;
    messages.push({
      role: message.role,
      parts,
      ...(message.name ? { name: message.name } : {}),
      ...(message.finish_reason ? { finishReason: message.finish_reason } : {}),
    });
  }
  return messages;
}

/** System instructions as parts, or `null` where they are not the conventions' shape. */
export function instructionsOf(source: string): Part[] | null {
  const parsed = z.array(z.unknown()).safeParse(json(source));
  return parsed.success ? partsIn(parsed.data) : null;
}

/** JSON indented to read, or the text as it arrived where it is not JSON. */
export function readable(source: string): string {
  const parsed = json(source);
  return parsed === undefined ? source : JSON.stringify(parsed, null, 2);
}

/** A part's JSON value indented to read: a string as it is, anything else as JSON. */
export function readableValue(value: unknown): string {
  return typeof value === "string" ? readable(value) : JSON.stringify(value, null, 2);
}

/** The fields an SDK left out to fit an export, by name: none where it cut nothing. */
export function cutFrom(fields: Readonly<Record<string, string>>): string[] {
  const cut = fields[ContentField.ContentCut];
  if (cut === undefined) return [];
  const parsed = z.array(z.string()).safeParse(json(cut));
  return parsed.success ? parsed.data : [cut];
}

/** The attribute an SDK's scrubbers count what they replaced in, by kind
 *  (`crates/telemetry/src/contract.rs`, `SCRUBBED`). */
export const SCRUBBED = "telmoni.scrubbed";

/** Each kind a scrubber replaces, as its marker names it. */
export const SCRUBBED_KIND: Readonly<Record<string, string>> = {
  "secret:private_key": "private key",
  "secret:connection_string": "connection string",
  "secret:jwt": "JWT",
  "secret:anthropic_key": "Anthropic key",
  "secret:openai_key": "OpenAI key",
  "secret:stripe_key": "Stripe key",
  "secret:github_token": "GitHub token",
  "secret:slack_token": "Slack token",
  "secret:aws_key": "AWS key",
  "secret:gcp_key": "Google Cloud key",
  "secret:telmoni_key": "Telmoni key",
  "secret:password": "password",
  "secret:token": "token",
  "pii:email": "email address",
  "pii:iban": "IBAN",
  "pii:card": "card number",
  "pii:ssn": "social security number",
  "pii:phone": "phone number",
  "pii:ip": "IP address",
  "pii:home_path": "home folder",
};

/** What the SDK or door replaced, and the fields dropped on failure. */
export function scrubbedFrom(
  attributes: Readonly<Record<string, string>>,
): { replaced: string[]; dropped: string[] } | null {
  const raw = attributes[SCRUBBED];
  if (raw === undefined) return null;
  const parsed = z.record(z.string(), z.number().int().positive()).safeParse(json(raw));
  if (!parsed.success) return null;
  const replaced: string[] = [];
  const dropped: string[] = [];
  for (const [key, count] of Object.entries(parsed.data).sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    if (key.startsWith("dropped:")) {
      dropped.push(key.slice("dropped:".length));
      continue;
    }
    const kind = SCRUBBED_KIND[key] ?? key;
    const many = kind.endsWith("s") ? `${kind}es` : `${kind}s`;
    replaced.push(`${count} ${count === 1 ? kind : many}`);
  }
  return { replaced, dropped };
}

/** How the trace names each content field it draws, in the order it draws them. */
export const FIELD_LABEL: ReadonlyArray<readonly [ContentField, string]> = [
  [ContentField.SystemInstructions, "System instructions"],
  [ContentField.ToolDefinitions, "Tools offered"],
  [ContentField.InputMessages, "Input"],
  [ContentField.OutputMessages, "Output"],
  [ContentField.ToolCallArguments, "Arguments"],
  [ContentField.ToolCallResult, "Result"],
  [ContentField.RetrievalQueryText, "Query"],
  [ContentField.RetrievalDocuments, "Documents"],
  [ContentField.MemoryQueryText, "Memory query"],
  [ContentField.MemoryRecords, "Memory records"],
  [ContentField.PromptVariable, "Prompt variables"],
  [ContentField.UserName, "User name"],
  [ContentField.UserFullName, "User's full name"],
  [ContentField.UserEmail, "User email"],
  [ContentField.Events, "Events"],
];

/** The fields a failure writes, drawn together above the rest. */
const FAILURE_FIELDS: readonly string[] = [
  ContentField.ExceptionType,
  ContentField.ExceptionMessage,
  ContentField.ExceptionStacktrace,
  ContentField.StatusMessage,
];

/** A failure as the span kept it: its class, its message — the exception's,
 *  else the status message — and its stack trace; none where it kept none. */
export function failureOf(
  fields: Readonly<Record<string, string>>,
): { type?: string; message?: string; stacktrace?: string } | null {
  const message = fields[ContentField.ExceptionMessage] ?? fields[ContentField.StatusMessage];
  const type = fields[ContentField.ExceptionType];
  const stacktrace = fields[ContentField.ExceptionStacktrace];
  if (message === undefined && type === undefined && stacktrace === undefined) return null;
  return {
    ...(type !== undefined && { type }),
    ...(message !== undefined && { message }),
    ...(stacktrace !== undefined && { stacktrace }),
  };
}

/** The fields a span kept that no label names: its metadata, kept as content. */
export function metadataOf(fields: Readonly<Record<string, string>>): [string, string][] {
  const named = new Set<string>([
    ...FIELD_LABEL.map(([field]) => field),
    ...FAILURE_FIELDS,
    ContentField.ContentCut,
  ]);
  return Object.entries(fields)
    .filter(([name]) => !named.has(name))
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

/** A span's kept fields: `null` where the content could not be read,
 *  `undefined` where the span kept none. */
export function fieldsOf(
  content: readonly SpanFields[] | null,
  spanId: string,
): Readonly<Record<string, string>> | null | undefined {
  if (content === null) return null;
  return content.find((span) => span.span_id === spanId)?.fields;
}

/** Whether `text` names a file as the server keeps one: the SHA-256 of its
 *  bytes in lowercase hex. */
export function isFileHash(text: string): boolean {
  return /^[0-9a-f]{64}$/.test(text);
}

/** The hash of the file a part names as kept here, or none. */
export function keptFileOf(part: Part): string | null {
  if (part.type !== "media" || part.kind !== "file") return null;
  if (!part.fileId?.startsWith(Content.fileIdPrefix)) return null;
  const hash = part.fileId.slice(Content.fileIdPrefix.length);
  return isFileHash(hash) ? hash : null;
}

/** How a kept file is drawn: an image the browser shows inline, audio it
 *  plays, and anything else, SVG among it, a download. */
export function drawnAs(mimeType: string | undefined): "image" | "audio" | "download" {
  if (mimeType && ["image/png", "image/jpeg", "image/gif", "image/webp"].includes(mimeType)) {
    return "image";
  }
  if (mimeType?.startsWith("audio/")) return "audio";
  return "download";
}
