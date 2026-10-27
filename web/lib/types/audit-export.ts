import { z } from "zod";

import { ExportFailure, ExportFormat, ExportStatus } from "@/lib/types/enums";

/// One export of the organization's audit log, as auth lists it for the
/// person who asked for it: never the file itself, which is fetched alone.
export const AuditExportSchema = z.object({
  id:            z.string(),
  format:        z.enum(ExportFormat),
  /// `null` from the chain's first row.
  range_from:    z.string().nullable(),
  range_to:      z.string(),
  status:        z.enum(ExportStatus),
  /// Why a failed one failed: more rows than one file holds, or an error.
  failure:       z.enum(ExportFailure).nullable(),
  row_count:     z.number().nullable(),
  bytes:         z.number().nullable(),
  created_at:    z.string(),
  finished_at:   z.string().nullable(),
  expires_at:    z.string().nullable(),
  downloaded_at: z.string().nullable(),
});
export type AuditExport = z.infer<typeof AuditExportSchema>;

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
export function isBuilding(entry: Pick<AuditExport, "status">): boolean {
  return entry.status === "queued" || entry.status === "running";
}

/// Ready and not yet taken: what the bell offers.
export function isWaiting(entry: Pick<AuditExport, "status" | "downloaded_at">): boolean {
  return entry.status === "ready" && entry.downloaded_at === null;
}
