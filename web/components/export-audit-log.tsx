"use client";

import { Download, Loader2 } from "lucide-react";
import { useId, useState, useTransition } from "react";
import { toast } from "sonner";
import { z } from "zod";

import { downloadExport } from "@/components/audit-exports";
import { ChoiceGroup, type Choice } from "@/components/choice-group";
import { PageAction } from "@/components/page-action";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  dayField,
  exportSummary,
  exportWindow,
  exportsHref,
  type ExportRange,
} from "@/lib/audit-exports";
import {
  useActiveOrganization,
  useAddAuditExport,
  useAuditExports,
  useForgetAuditExport,
  useMarkAuditExportDownloaded,
} from "@/lib/store";
import {
  AUDIT_EXPORT_KEPT_DAYS,
  AuditExportSchema,
  isBuilding,
  type AuditExportFormat,
} from "@/lib/types/audit-export";

const RANGES: readonly Choice<ExportRange>[] = [
  { value: "7d", label: "7 days" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
  { value: "all", label: "All time" },
  { value: "custom", label: "Custom" },
];

const FORMATS: readonly Choice<AuditExportFormat>[] = [
  { value: "json", label: "JSON" },
  { value: "csv", label: "CSV" },
];

const FORMAT_HINTS: Record<AuditExportFormat, string> = {
  json: "Every event with its hashes, for the verify script in the docs.",
  csv: "The same events, for a spreadsheet. Verify with the JSON.",
};

/** Starting one is a row written and a task spawned; the build is not waited on. */
const START_TIMEOUT_MS = 15_000;

const StartedSchema = z.object({ export: AuditExportSchema });

/** How many of the person's exports the dialog lists under its choices. */
const RECENT = 3;

const FAILED_TO_START = "The export could not be started. Try again in a moment.";

// Export: the page's primary action, on `c` as every page's is. It opens the
// choices — how much of the log, and in which format — and the file is built
// in the background: the dialog closes at once, and the bell holds the file
// once it is ready until it is taken, from a file auth keeps for a week. Each
// file is recorded on the chain as an export.
export function ExportAuditLog({ organization }: { organization: string }) {
  const [open, setOpen] = useState(false);
  // A fresh dialog each time it opens: today as it is then — a page left open
  // past midnight otherwise could not pick the new day — and nothing left over
  // from the last time, a refusal or a half-picked range.
  const [opened, setOpened] = useState(0);
  return (
    <>
      <PageAction
        primary="Export"
        onClick={() => {
          setOpened((n) => n + 1);
          setOpen(true);
        }}
      >
        <Download className="size-4" />
        Export
      </PageAction>
      <ExportDialog
        key={opened}
        organization={organization}
        open={open}
        onOpenChange={setOpen}
      />
    </>
  );
}

function ExportDialog({
  organization,
  open,
  onOpenChange,
}: {
  organization: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const active = useActiveOrganization();
  const addAuditExport = useAddAuditExport();
  const [range, setRange] = useState<ExportRange>("30d");
  const [format, setFormat] = useState<AuditExportFormat>("json");
  const [today] = useState(() => dayField(new Date()));
  const [fromDay, setFromDay] = useState("");
  const [toDay, setToDay] = useState(today);
  const [error, setError] = useState<string | null>(null);
  const [pending, start] = useTransition();
  const id = useId();

  // `yyyy-mm-dd` compares as the days it names.
  const customWhole = fromDay !== "" && toDay !== "" && fromDay <= toDay && fromDay <= today;
  const canSubmit = !pending && (range !== "custom" || customWhole);

  function submit() {
    const bounds = exportWindow(range, fromDay, toDay, new Date());
    if (!bounds) {
      setError("Pick a first day no later than the last, and not after today.");
      return;
    }
    setError(null);
    start(async () => {
      try {
        const res = await fetch(exportsHref(organization), {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ format, ...bounds }),
          // Signed out, the proxy answers with the sign-in page's redirect,
          // which a POST followed there would only be refused by.
          redirect: "manual",
          signal: AbortSignal.timeout(START_TIMEOUT_MS),
        });
        if (res.type === "opaqueredirect" || res.status === 401) {
          setError("Your session has ended. Sign in again to export the audit log.");
          return;
        }
        if (res.status === 409) {
          setError("Three of your exports are still being built. Start this one once one of them is ready.");
          return;
        }
        if (res.status === 429) {
          setError("You’ve started a lot of exports this hour. Try again a little later.");
          return;
        }
        if (res.status === 403) {
          setError("Only an owner or admin can export the audit log.");
          return;
        }
        // The days the person picked, measured against the server's clock.
        if (res.status === 400) {
          setError("Pick a range that starts before it ends, and no later than today.");
          return;
        }
        const parsed = res.ok ? StartedSchema.safeParse(await res.json()) : null;
        if (!parsed?.success) {
          setError(FAILED_TO_START);
          return;
        }
        // Into the store before the bell's next read, so the bell shows it
        // building at once and announces it when it is done.
        if (active) addAuditExport(active.organizationId, parsed.data.export);
        onOpenChange(false);
        toast("Preparing your export.", {
          description: "The bell will hold it when it’s ready.",
        });
      } catch {
        setError(FAILED_TO_START);
      }
    });
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        // Held open while a start is out: closed, its answer — the export it
        // queued, or why not — would land in a dialog that is gone.
        if (!next && pending) return;
        onOpenChange(next);
        if (!next) setError(null);
      }}
    >
      <DialogContent className="sm:max-w-md">
        <form
          className="contents"
          onSubmit={(e) => {
            e.preventDefault();
            if (canSubmit) submit();
          }}
        >
          <DialogHeader>
            <DialogTitle>Export audit log</DialogTitle>
          </DialogHeader>
          <DialogBody>
            <DialogDescription>
              Built in the background, with every event&rsquo;s hashes. When
              it&rsquo;s ready, the bell holds it until you download it, and the
              file is kept for {AUDIT_EXPORT_KEPT_DAYS} days.
            </DialogDescription>

            <div className="grid gap-2">
              <span id={`${id}-range`} className="text-sm font-medium">
                Range
              </span>
              {/* A row, so the group keeps its own width rather than the
                  grid's: its border is the outline of its segments. */}
              <div className="flex">
                <ChoiceGroup
                  labelledBy={`${id}-range`}
                  options={RANGES}
                  value={range}
                  onChange={setRange}
                />
              </div>
            </div>

            {range === "custom" && (
              <div className="grid grid-cols-2 gap-3">
                <div className="grid gap-2">
                  <Label htmlFor={`${id}-from`}>First day</Label>
                  <Input
                    id={`${id}-from`}
                    type="date"
                    value={fromDay}
                    max={toDay || today}
                    onChange={(e) => setFromDay(e.target.value)}
                    required
                  />
                </div>
                <div className="grid gap-2">
                  <Label htmlFor={`${id}-to`}>Last day</Label>
                  <Input
                    id={`${id}-to`}
                    type="date"
                    value={toDay}
                    min={fromDay || undefined}
                    max={today}
                    onChange={(e) => setToDay(e.target.value)}
                    required
                  />
                </div>
              </div>
            )}

            <div className="grid gap-2">
              <span id={`${id}-format`} className="text-sm font-medium">
                Format
              </span>
              <div className="flex">
                <ChoiceGroup
                  labelledBy={`${id}-format`}
                  options={FORMATS}
                  value={format}
                  onChange={setFormat}
                />
              </div>
              <p className="text-xs text-muted-foreground">{FORMAT_HINTS[format]}</p>
            </div>

            <RecentExports organization={organization} />

            {error && (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
          </DialogBody>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              disabled={pending}
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={!canSubmit}>
              {pending ? "Starting…" : "Start export"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

// The person's latest exports of this organization, so the dialog that
// starts one also says what became of the last: still building, ready to
// take again within its week, or failed and why.
function RecentExports({ organization }: { organization: string }) {
  const active = useActiveOrganization();
  const exports = useAuditExports();
  const markDownloaded = useMarkAuditExportDownloaded();
  const forget = useForgetAuditExport();
  const [now] = useState(() => new Date());

  const recent = active ? (exports[active.organizationId] ?? []).slice(0, RECENT) : [];
  if (recent.length === 0) return null;

  return (
    <div className="grid gap-2">
      <span className="text-sm font-medium">Your recent exports</span>
      <ul className="grid gap-1">
        {recent.map((entry) => (
          <li key={entry.id} className="flex min-h-9 items-center justify-between gap-3 text-sm">
            <span className="min-w-0 truncate text-muted-foreground">
              {exportSummary(entry, now)}
            </span>
            {entry.status === "ready" ? (
              <Button
                type="button"
                variant="outline"
                size="sm"
                aria-label={`Download ${exportSummary(entry, now)}`}
                onClick={() => {
                  void downloadExport(organization, entry.id).then((result) => {
                    if (result === "taken") markDownloaded(entry.id);
                    if (result === "gone") forget(entry.id);
                  });
                }}
              >
                <Download className="size-4" />
                Download
              </Button>
            ) : isBuilding(entry) ? (
              <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                <Loader2 aria-hidden className="size-3.5 animate-spin" />
                Preparing
              </span>
            ) : (
              <span className="shrink-0 text-xs text-destructive">
                {entry.failure === "too_large" ? "Too large" : "Failed"}
              </span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
