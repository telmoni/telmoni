import type { ReactNode } from "react";

import { LocalTime } from "@/components/local-time";
import { MonitorStateBadge } from "@/components/monitors/state-badge";
import { Row, Rows } from "@/components/rows";
import { Card } from "@/components/ui/card";
import { boardOf, countedLine, lastRunLine } from "@/lib/monitors";
import type { Monitor, MonitorsAccess } from "@/lib/server/data";

// The project's home: the monitors that need someone, the worst first, then
// those running, each with its last run, and every other counted in one
// line. Read from Postgres alone, each monitor keeping its last run as the
// run ends, so nothing is summed as the page is drawn; drawn again on the
// ping lane's nudge (`realtime-listener.tsx`), never on a timer. A board of
// runs and their bounds: no uptime grid and no percentage. Telemetry
// unreadable is said, never hidden: a board gone quiet would read as every
// monitor well.
export function StatusBoard({ monitors }: { monitors: MonitorsAccess }) {
  if (monitors.kind === "forbidden") {
    return (
      <Section>
        <Note>You can&apos;t read this project&apos;s monitors.</Note>
      </Section>
    );
  }
  if (monitors.kind === "unavailable") {
    return (
      <Section>
        <Note>Monitors are unavailable right now — try again shortly.</Note>
      </Section>
    );
  }
  if (monitors.monitors.length === 0) {
    return (
      <Section>
        <Card className="gap-0 overflow-hidden p-0">
          <div className="grid gap-1 p-4">
            <p className="text-sm font-medium">No monitors yet.</p>
            <p className="text-sm text-muted-foreground">
              A monitor watches one agent&apos;s runs: when they are expected, how long one may
              go quiet, how many steps it may take and what it may spend. The first ping to a
              slug makes one.
            </p>
          </div>
        </Card>
      </Section>
    );
  }
  const board = boardOf(monitors.monitors);
  const rows = [...board.needing, ...board.running];
  return (
    <Section counted={countedLine(board.counted)}>
      {rows.length === 0 ? (
        <Note>Nothing needs anyone, and no run is going.</Note>
      ) : (
        <Rows>
          {rows.map((monitor) => (
            <MonitorRow key={monitor.id} monitor={monitor} />
          ))}
        </Rows>
      )}
    </Section>
  );
}

// The section's heading row, the rest of the board counted at its right.
function Section({ counted, children }: { counted?: string; children: ReactNode }) {
  return (
    <section className="grid gap-3" aria-label="Monitors">
      <div className="flex items-center justify-between gap-2">
        <h2 className="text-sm font-medium">Monitors</h2>
        {counted ? <p className="text-sm text-muted-foreground">{counted}</p> : null}
      </div>
      {children}
    </section>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <Card className="gap-0 p-4 text-sm text-muted-foreground">{children}</Card>;
}

function MonitorRow({ monitor }: { monitor: Monitor }) {
  const run = monitor.last_run;
  const line = lastRunLine(monitor);
  return (
    <Row
      label={
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate">{monitor.name}</span>
          <MonitorStateBadge state={monitor.state} />
        </span>
      }
      hint={
        run ? (
          <>
            Last run <LocalTime iso={run.ended_at} mode="datetime" />
            {line ? ` · ${line}` : null}
          </>
        ) : (
          "No run has ended yet"
        )
      }
    >
      <span className="text-muted-foreground">
        Since <LocalTime iso={monitor.state_changed_at} mode="datetime" />
      </span>
    </Row>
  );
}
