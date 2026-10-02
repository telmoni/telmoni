import { NotificationKind } from "@/lib/types/enums";

// Keyed by the generated vocabulary, so a kind the Rust enum gains fails the
// typecheck here until it has a label.
const LABELS = {
  member_added: "Member added",
  member_left: "Member left",
  organization_alert: "Organization alert",
  connector_connected: "Channel connected",
  connector_disconnected: "Channel disconnected",
} as const satisfies Record<NotificationKind, string>;

export const NOTIFICATION_KINDS: readonly NotificationKind[] = Object.values(NotificationKind);

const KINDS: ReadonlySet<string> = new Set(NOTIFICATION_KINDS);

export function isNotificationKind(value: unknown): value is NotificationKind {
  return typeof value === "string" && KINDS.has(value);
}

export function eventKindLabel(kind: string): string {
  return isNotificationKind(kind) ? LABELS[kind] : kind.replace(/_/g, " ");
}
