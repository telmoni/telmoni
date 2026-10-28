"use client";

import { useCallback, useEffect, useRef } from "react";
import { toast } from "sonner";

import { ProblemType, problemType } from "@/lib/api/problem";
import { exportHref, exportSummary, exportsHref, failureText } from "@/lib/audit-exports";
import { administers } from "@/lib/roles";
import { routeReadOf, signedOut } from "@/lib/route-read";
import {
  useActiveOrganization,
  useAuditExports,
  useAuditExportsStarted,
  useForgetAuditExport,
  useMarkAuditExportDownloaded,
  useReplaceAuditExports,
} from "@/lib/store";
import {
  AuditExportListSchema,
  isBuilding,
  type AuditExport,
  type AuditExportLimits,
} from "@/lib/types/audit-export";

/// How often an export still building is asked after: most finish inside a
/// second or two, so the wait for the toast is about one of these.
const POLL_MS = 2_000;

const READ_TIMEOUT_MS = 10_000;

/// Reads in a row that may fail before the polling stops: past it the server
/// is down or the session is gone, and asking every two seconds helps neither.
/// The tab coming back asks again.
const FAILURES_BEFORE_STALLING = 3;

/// How long the file may take to start arriving: past the console's own
/// minute to fetch it from auth, so the console answers first and this hears
/// why.
const DOWNLOAD_HEADERS_TIMEOUT_MS = 65_000;

/// How long the file may take once it is arriving: a whole file over a slow
/// phone connection, which one deadline on the whole exchange cut off.
const DOWNLOAD_BODY_TIMEOUT_MS = 10 * 60_000;

/// How long the file's address outlives the click that hands it over: past a
/// browser that asks where to save it first, as Safari on iOS does.
const REVOKE_AFTER_MS = 40_000;

/// A download slower than this says it is under way.
const SLOW_DOWNLOAD_MS = 1_000;

/// The downloads under way, by export: a second press waits for the first
/// rather than fetching the file again and saving it twice.
const downloading = new Set<string>();

/**
 * Fetch a finished file and hand it to the browser, saying what went wrong
 * when it cannot: a file past its week, a role lost since it was built, a
 * session ended. Fetched rather than linked, so a refusal is a toast here
 * instead of a download of an error page. Answers what became of it: `taken`,
 * `gone` when it is no longer kept, `busy` when a download of it is already
 * under way, or `failed`.
 */
/** Takes an export: downloaded, then no longer offered — marked taken once
 *  the browser has the file, forgotten once it is gone. One function for the
 *  bell, the export dialog and the ready toast. */
export function useTakeExport(): (organization: string, id: string) => void {
  const markDownloaded = useMarkAuditExportDownloaded();
  const forget = useForgetAuditExport();
  return useCallback(
    (organization: string, id: string) => {
      void downloadExport(organization, id).then((result) => {
        if (result === "taken") markDownloaded(id);
        if (result === "gone") forget(id);
      });
    },
    [markDownloaded, forget],
  );
}

export async function downloadExport(
  organizationSlug: string,
  id: string,
): Promise<"taken" | "gone" | "busy" | "failed"> {
  if (downloading.has(id)) return "busy";
  downloading.add(id);
  const controller = new AbortController();
  let deadline = setTimeout(() => controller.abort(), DOWNLOAD_HEADERS_TIMEOUT_MS);
  let slow: string | number | undefined;
  const slowTimer = setTimeout(() => {
    slow = toast.loading("Downloading your audit log export…");
  }, SLOW_DOWNLOAD_MS);
  try {
    const res = await fetch(exportHref(organizationSlug, id), {
      cache: "no-store",
      redirect: "manual",
      signal: controller.signal,
    });
    if (signedOut(res)) {
      toast.error("Your session has ended. Sign in again to download it.");
      return "failed";
    }
    if (!res.ok) {
      // The organization's day of data out, as against this person's hour.
      const spent =
        res.status === 429 &&
        problemType(await res.json().catch(() => null)) === ProblemType.TenantRateLimited;
      toast.error(
        res.status === 404
          ? "That export is no longer kept. Start a new one."
          : res.status === 403
            ? "Only an owner or admin can download the audit log."
            : spent
              ? "This organization has downloaded all its plan holds today. Try again after midnight UTC."
              : res.status === 429
                ? "You’ve downloaded a lot of exports this hour. Try again a little later."
                : "The export could not be downloaded. Try again in a moment.",
      );
      return res.status === 404 ? "gone" : "failed";
    }
    clearTimeout(deadline);
    deadline = setTimeout(() => controller.abort(), DOWNLOAD_BODY_TIMEOUT_MS);
    const name =
      /filename="([^"]+)"/.exec(res.headers.get("content-disposition") ?? "")?.[1] ??
      "audit-log";
    const url = URL.createObjectURL(await res.blob());
    const link = document.createElement("a");
    link.href = url;
    link.download = name;
    document.body.appendChild(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), REVOKE_AFTER_MS);
    return "taken";
  } catch {
    toast.error("The export could not be downloaded. Try again in a moment.");
    return "failed";
  } finally {
    clearTimeout(deadline);
    clearTimeout(slowTimer);
    if (slow !== undefined) toast.dismiss(slow);
    downloading.delete(id);
  }
}

/**
 * Keeps the store's list of the person's audit log exports current for the
 * organization the console stands in, when they own or administer it: read
 * once the console is on screen, again whenever the tab comes back, and every
 * two seconds while one is building. Mounted with the bell, which is on every
 * page, so the toast that says a file is ready finds the person wherever in
 * the console they went after starting it — and, the lists being kept per
 * organization, when they come back to the one it was built in.
 */
export function useAuditExportsWatcher(): void {
  const organization = useActiveOrganization();
  const exports = useAuditExports();
  const started = useAuditExportsStarted();
  const replaceAuditExports = useReplaceAuditExports();
  const take = useTakeExport();

  const watched =
    organization && administers(organization.role) ? organization : null;
  const slug = watched?.slug ?? null;
  const organizationId = watched?.organizationId ?? null;
  const building =
    organizationId !== null && (exports[organizationId] ?? []).some(isBuilding);

  // ⚠ **The newest read's answer wins, whatever order they land in.** Reads
  // overlap — the tab coming back while a poll is out — and an older list
  // landing last put a finished export back to building, and announced it
  // again when the next poll found it done.
  const issued = useRef(0);
  const answered = useRef(0);
  const failures = useRef(0);
  // Reads still out: a poll waits for them rather than piling more onto a
  // server slow to answer.
  const reading = useRef(0);

  // The starts a read leaves with. Behind by a render at worst, which only
  // makes the store turn that read's answer away, and the polling the start
  // began reads again. A start is the server answering, too: whatever had
  // stalled the polling is past, so it polls again from here.
  const startedRef = useRef(started);
  useEffect(() => {
    startedRef.current = started;
    failures.current = 0;
  }, [started]);

  const read = useCallback(async () => {
    if (!slug || !organizationId) return;
    const ticket = ++issued.current;
    const startedBefore = startedRef.current;
    let fresh: AuditExport[];
    let limits: AuditExportLimits;
    reading.current += 1;
    try {
      const res = await fetch(exportsHref(slug), {
        cache: "no-store",
        redirect: "manual",
        signal: AbortSignal.timeout(READ_TIMEOUT_MS),
      });
      const read = await routeReadOf(res, AuditExportListSchema);
      // No longer theirs to read: the session ended, the role went, or the
      // organization did. Nothing of it is building for them any more, so
      // nothing to poll.
      if (read.kind === "signed-out" || read.kind === "forbidden" || read.kind === "missing") {
        if (ticket >= answered.current) {
          replaceAuditExports(organizationId, [], startedBefore);
        }
        return;
      }
      if (read.kind !== "ok") throw new Error(`exports answered ${read.status}`);
      fresh = read.value.exports;
      limits = read.value;
    } catch {
      // The list stays as it was; enough of these in a row stop the polling.
      failures.current += 1;
      return;
    } finally {
      reading.current -= 1;
    }
    failures.current = 0;
    if (ticket < answered.current) return;
    const before = replaceAuditExports(organizationId, fresh, startedBefore);
    if (before === null) return;
    answered.current = ticket;

    // What the store held as building and this list says is done.
    for (const entry of fresh) {
      const was = before.find((e) => e.id === entry.id);
      if (!was || !isBuilding(was) || isBuilding(entry)) continue;
      if (entry.status === "ready") {
        toast.success("Your audit log export is ready.", {
          description: exportSummary(entry, new Date()),
          action: {
            label: "Download",
            onClick: () => take(slug, entry.id),
          },
        });
      } else {
        toast.error(failureText(entry, limits));
      }
    }
  }, [slug, organizationId, replaceAuditExports, take]);

  useEffect(() => {
    if (!slug) return;
    void read();
    const onVisible = () => {
      if (document.visibilityState === "visible") void read();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [slug, read]);

  useEffect(() => {
    if (!building) return;
    const timer = setInterval(() => {
      // Stalled after failures in a row; a read that succeeds, the tab
      // coming back's included, or a start sets it going again.
      if (reading.current === 0 && failures.current < FAILURES_BEFORE_STALLING) void read();
    }, POLL_MS);
    return () => clearInterval(timer);
  }, [building, read]);
}
