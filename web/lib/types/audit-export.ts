import { z } from "zod";

import { ExportFailure, ExportFormat, ExportStatus } from "@/lib/types/enums";

/// What every export carries, whatever state it is in.
const Asked = {
  id:         z.string(),
  format:     z.enum(ExportFormat),
  /// `null` from the chain's first row.
  range_from: z.string().nullable(),
  range_to:   z.string(),
  created_at: z.string(),
};

/// One export of the organization's audit log, as auth lists it for the
/// person who asked for it: never the file itself, which is fetched alone.
/// Each state carries what it has, as auth's `ExportState` does: a finished
/// one when it finished and until when it is kept, a ready one its file's
/// rows and bytes and whether it was taken, a failed one why.
export const AuditExportSchema = z.discriminatedUnion("status", [
  z.object({ ...Asked, status: z.literal(ExportStatus.Queued) }),
  z.object({ ...Asked, status: z.literal(ExportStatus.Running) }),
  z.object({
    ...Asked,
    status:        z.literal(ExportStatus.Ready),
    row_count:     z.number(),
    bytes:         z.number(),
    finished_at:   z.string(),
    expires_at:    z.string(),
    downloaded_at: z.string().nullable(),
  }),
  z.object({
    ...Asked,
    status:      z.literal(ExportStatus.Failed),
    /// More rows than one file holds, or an error.
    failure:     z.enum(ExportFailure),
    finished_at: z.string(),
    expires_at:  z.string(),
  }),
]);
export type AuditExport = z.infer<typeof AuditExportSchema>;
export type ReadyAuditExport = Extract<AuditExport, { status: "ready" }>;
export type FailedAuditExport = Extract<AuditExport, { status: "failed" }>;

export type AuditExportFormat = AuditExport["format"];

/// What one file may hold, as auth answers it with the list: the rows, and the
/// bytes, which the organization's plan may hold lower. Past either, an export
/// fails as `too_large`, and a shorter range is the answer.
export const AuditExportLimitsSchema = z.object({
  max_rows:  z.number().int().positive(),
  max_bytes: z.number().int().positive(),
});
export type AuditExportLimits = z.infer<typeof AuditExportLimitsSchema>;

/// The caller's exports, newest first, with what one file may hold.
export const AuditExportListSchema = AuditExportLimitsSchema.extend({
  exports: z.array(AuditExportSchema),
});

/// How long a finished file is kept for the person who asked, as auth's
/// `KEEP_FOR_SECS` keeps it.
export const AUDIT_EXPORT_KEPT_DAYS = 7;

/// Still being built: queued, or a build holds it.
export function isBuilding(
  entry: AuditExport,
): entry is Extract<AuditExport, { status: "queued" | "running" }> {
  return entry.status === "queued" || entry.status === "running";
}

/// Ready and not yet taken: what the bell offers.
export function isWaiting(entry: AuditExport): entry is ReadyAuditExport {
  return entry.status === "ready" && entry.downloaded_at === null;
}
