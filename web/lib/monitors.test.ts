import { describe, expect, it } from "vitest";

import {
  blankForm,
  boardOf,
  boundSources,
  boundsInBrief,
  changeLine,
  checkForm,
  countedLine,
  cronInWords,
  everyPeriod,
  filterMonitors,
  formOf,
  formatSpend,
  lastRunLine,
  monitorHref,
  monitorsHref,
  parseMonitorQuery,
  reportLines,
  scheduleInWords,
  slugFromName,
  stood,
  typedOf,
} from "./monitors";
import type { Monitor } from "./server/entities/monitors";
import { MonitorState } from "./types/enums";

const { Failed, Stalled, OverBound, Late, Running, New, Paused, Healthy } = MonitorState;

function inStates(...states: MonitorState[]) {
  return states.map((state, i) => ({ id: `m${i}`, state }));
}

describe("the board", () => {
  it("lists the monitors that need someone, then those running, and counts the rest", () => {
    const board = boardOf(
      inStates(Failed, Stalled, OverBound, Late, Running, New, Paused, Healthy, Healthy),
    );
    expect(board.needing.map((m) => m.state)).toEqual([Failed, Stalled, OverBound, Late]);
    expect(board.running.map((m) => m.state)).toEqual([Running]);
    expect(board.counted).toEqual([
      { state: New, count: 1 },
      { state: Paused, count: 1 },
      { state: Healthy, count: 2 },
    ]);
  });

  it("keeps the order it was given, which is the lane's: the worst first", () => {
    const board = boardOf(inStates(Late, Failed, Running, Running));
    expect(board.needing.map((m) => m.id)).toEqual(["m0", "m1"]);
    expect(board.running.map((m) => m.id)).toEqual(["m2", "m3"]);
  });

  it("counts in one line, a state with none left out", () => {
    expect(countedLine([{ state: Healthy, count: 12 }])).toBe("12 healthy");
    expect(countedLine([{ state: New, count: 2 }, { state: Healthy, count: 1234 }])).toBe(
      "2 new · 1,234 healthy",
    );
    expect(boardOf([]).counted).toEqual([]);
    expect(countedLine([])).toBe("");
  });
});

describe("a monitor's last run", () => {
  const bounds = { stall_after_s: null, max_steps: null, max_cost_usd: null };
  const run = {
    run_id: "5d4f5c68-0000-4000-8000-000000000001",
    started_at: "2026-10-21T03:00:00Z",
    ended_at: "2026-10-21T03:04:02Z",
    status: "ok" as const,
    steps: 12,
    cost_usd: 0.08,
  };

  it("reads as its verdict, its length, and its steps and spend against the caps", () => {
    expect(lastRunLine({ last_run: run, bounds })).toBe("OK · 4 min 2 s · 12 steps · $0.08");
    expect(
      lastRunLine({ last_run: run, bounds: { ...bounds, max_steps: 40, max_cost_usd: 2 } }),
    ).toBe("OK · 4 min 2 s · 12 / 40 steps · $0.08 / $2.00");
  });

  it("leaves out what the run did not report, and the length of a bare ping", () => {
    const bare = { ...run, ended_at: run.started_at, status: "error" as const };
    expect(lastRunLine({ last_run: { ...bare, steps: null, cost_usd: null }, bounds })).toBe(
      "Error",
    );
    expect(lastRunLine({ last_run: { ...run, steps: 1, cost_usd: null }, bounds })).toBe(
      "OK · 4 min 2 s · 1 step",
    );
  });

  it("is nothing until a run has ended", () => {
    expect(lastRunLine({ last_run: null, bounds })).toBeNull();
  });

  it("writes spend to the cent, and below a cent to the hundredth of one", () => {
    expect(formatSpend(0.08)).toBe("$0.08");
    expect(formatSpend(0.0012)).toBe("$0.0012");
    expect(formatSpend(1234.5)).toBe("$1,234.50");
    expect(formatSpend(0)).toBe("$0.00");
  });
});

describe("a schedule in words", () => {
  it("words the cron lines a crontab most often holds", () => {
    expect(cronInWords("0 2 * * *")).toBe("every day at 02:00");
    expect(cronInWords("30 9 * * 1-5")).toBe("every weekday at 09:30");
    expect(cronInWords("0 18 * * MON")).toBe("every Monday at 18:00");
    expect(cronInWords("0 6 * * 1,3,5")).toBe("every Monday, Wednesday and Friday at 06:00");
    expect(cronInWords("0 0 * * 0,6")).toBe("every Saturday and Sunday at 00:00");
    expect(cronInWords("0 0 * * 0-6")).toBe("every day at 00:00");
    expect(cronInWords("15 4 1 * *")).toBe("on the 1st of every month at 04:15");
    expect(cronInWords("15 4 22 * *")).toBe("on the 22nd of every month at 04:15");
    expect(cronInWords("*/15 * * * *")).toBe("every 15 minutes");
    expect(cronInWords("* * * * *")).toBe("every minute");
    expect(cronInWords("0 * * * *")).toBe("every hour");
    expect(cronInWords("5 * * * *")).toBe("every hour at :05");
    expect(cronInWords("0 */6 * * *")).toBe("every 6 hours");
    expect(cronInWords("@daily")).toBe("every day at 00:00");
    expect(cronInWords("@HOURLY")).toBe("every hour");
  });

  it("leaves any other line unworded rather than word it wrong", () => {
    for (const line of [
      "0 2 * 1 *",
      "0 2 1,15 * *",
      "0 2 1 * 1",
      "0-30 * * * *",
      "0 24 * * *",
      "61 2 * * *",
      "*/0 * * * *",
      "0 2 * * mon-fri/2",
      "0 2 * *",
      "",
    ]) {
      expect(cronInWords(line), line).toBeNull();
    }
  });

  it("names the zone with a cron line, the interval alone, and none for neither", () => {
    const at = { schedule: null, interval_s: null, timezone: "Europe/London" };
    expect(scheduleInWords({ ...at, schedule: "0 2 * * *" })).toBe(
      "every day at 02:00, Europe/London",
    );
    expect(scheduleInWords({ ...at, schedule: "0 2 1,15 * *" })).toBe("0 2 1,15 * *, Europe/London");
    expect(scheduleInWords({ ...at, interval_s: 900 })).toBe("every 15 minutes");
    expect(scheduleInWords(at)).toBe("no schedule");
    expect(everyPeriod(3_600)).toBe("every hour");
    expect(everyPeriod(7_200)).toBe("every 2 hours");
    expect(everyPeriod(86_400)).toBe("every day");
    expect(everyPeriod(90)).toBe("every 90 seconds");
  });

  it("gives the bounds in brief, and nothing for none", () => {
    const none = { stall_after_s: null, max_steps: null, max_cost_usd: null };
    expect(boundsInBrief({ ...none, stall_after_s: 600, max_steps: 40 })).toBe("stall 10 min · 40 steps");
    expect(boundsInBrief({ ...none, max_steps: 1, max_cost_usd: 2 })).toBe("1 step · $2.00");
    expect(boundsInBrief(none)).toBeNull();
  });
});

const MONITOR: Monitor = {
  id: "0d2b7a6e-6c0f-4a1b-9f3e-1c2d3e4f5a6b",
  slug: "nightly-report",
  name: "Nightly report",
  agent: "report-bot",
  schedule: "0 2 * * *",
  interval_s: null,
  timezone: "Europe/London",
  grace_s: 600,
  expected_duration_s: 300,
  bounds: { stall_after_s: 120, max_steps: 40, max_cost_usd: 2.5 },
  state: MonitorState.Healthy,
  state_changed_at: "2026-10-21T02:10:00Z",
  next_due_at: "2026-10-22T01:00:00Z",
  last_run: null,
  last_success_at: null,
  created_at: "2026-10-01T00:00:00Z",
  updated_at: "2026-10-21T02:10:00Z",
};

describe("the list's query", () => {
  it("reads the states it knows and the words typed, and nothing else", () => {
    expect(parseMonitorQuery({})).toEqual({});
    expect(parseMonitorQuery({ state: ["late", "asleep", "failed", "late"], q: "  night  " })).toEqual({
      state: ["late", "failed"],
      q: "night",
    });
    expect(parseMonitorQuery({ q: "x".repeat(80) }).q).toHaveLength(60);
  });

  it("narrows by state and by words in the name, keeping the order given", () => {
    const monitors = [
      { name: "Nightly report", state: MonitorState.Failed },
      { name: "Hourly sync", state: MonitorState.Late },
      { name: "Night watch", state: MonitorState.Healthy },
    ];
    expect(filterMonitors(monitors, { q: "NIGHT" }).map((m) => m.name)).toEqual([
      "Nightly report",
      "Night watch",
    ]);
    expect(
      filterMonitors(monitors, { state: [MonitorState.Late, MonitorState.Healthy] }).map((m) => m.name),
    ).toEqual(["Hourly sync", "Night watch"]);
    expect(filterMonitors(monitors, { state: [MonitorState.Failed], q: "watch" })).toEqual([]);
  });

  it("writes the query into the page's URL, and a monitor's own page", () => {
    expect(monitorsHref("acme", "web", { state: [MonitorState.Late], q: "night" })).toBe(
      "/acme/web/monitors?state=late&q=night",
    );
    expect(monitorsHref("acme", "web", { q: "night" }, { q: undefined })).toBe("/acme/web/monitors");
    expect(monitorHref("acme", "web", "nightly-report")).toBe("/acme/web/monitors/nightly-report");
  });
});

describe("the monitor form", () => {
  it("makes the slug as the server does, from the name's Latin letters and digits", () => {
    expect(slugFromName("  Nightly report ")).toBe("nightly-report");
    expect(slugFromName("Ada's Café — v2")).toBe("adas-cafe-v2");
    expect(slugFromName("日報")).toBe("");
    expect(slugFromName(`${"word ".repeat(12)}end`), "cut at a word's end within 48").toBe(
      "word-word-word-word-word-word-word-word-word",
    );
  });

  it("types a length in its largest unit, and a monitor as its form edits it", () => {
    expect(typedOf(7_200)).toEqual({ value: "2", unit: "hours" });
    expect(typedOf(90)).toEqual({ value: "90", unit: "seconds" });
    expect(typedOf(0)).toEqual({ value: "0", unit: "minutes" });
    expect(typedOf(null)).toEqual({ value: "", unit: "minutes" });
    const form = formOf(MONITOR);
    expect(form).toMatchObject({
      timing: "cron",
      cron: "0 2 * * *",
      agent: "report-bot",
      grace: { value: "10", unit: "minutes" },
      stall: { value: "2", unit: "minutes" },
      steps: "40",
      cost: 2.5,
    });
  });

  it("sends what the lane takes, keeping a cost cap the form does not show", () => {
    expect(checkForm(formOf(MONITOR))).toEqual({
      ok: true,
      body: {
        name: "Nightly report",
        slug: "nightly-report",
        agent: "report-bot",
        schedule: "0 2 * * *",
        timezone: "Europe/London",
        grace_s: 600,
        expected_duration_s: 300,
        bounds: { stall_after_s: 120, max_steps: 40, max_cost_usd: 2.5 },
      },
    });
    const every = {
      ...blankForm("UTC"),
      name: "Hourly sync",
      slug: "hourly-sync",
      agent: "hourly-sync",
      timing: "interval" as const,
      every: { value: "1", unit: "hours" as const },
      grace: { value: "0", unit: "minutes" as const },
    };
    expect(checkForm(every)).toEqual({
      ok: true,
      body: { name: "Hourly sync", slug: "hourly-sync", interval_s: 3_600, grace_s: 0, bounds: {} },
    });
    expect(checkForm({ ...every, timing: "none" })).toEqual({
      ok: true,
      body: { name: "Hourly sync", slug: "hourly-sync", bounds: {} },
    });
  });

  it("names the first field a person must fix", () => {
    const form = { ...blankForm("UTC"), name: "Report", slug: "report" };
    const field = (over: Partial<typeof form>) => {
      const checked = checkForm({ ...form, ...over });
      return checked.ok ? null : checked.field;
    };
    expect(field({})).toBeNull();
    expect(field({ name: "  " })).toBe("name");
    expect(field({ slug: "" })).toBe("slug");
    expect(field({ cron: " " })).toBe("cron");
    expect(field({ grace: { value: "", unit: "minutes" } })).toBe("grace");
    expect(field({ timing: "interval", every: { value: "0", unit: "minutes" } })).toBe("every");
    expect(field({ expected: { value: "0.5", unit: "seconds" } })).toBe("expected");
    expect(field({ stall: { value: "-1", unit: "minutes" } })).toBe("stall");
    expect(field({ steps: "1.5" })).toBe("steps");
  });

  it("reports runs by the key's variable, never a key", () => {
    const lines = reportLines("nightly-report", "report-bot").map((l) => l.line);
    expect(lines).toEqual([
      'curl -fsS -m 10 "$TELMONI_ENDPOINT/ping/v1/$TELMONI_PING_KEY/nightly-report/start"',
      'curl -fsS -m 10 "$TELMONI_ENDPOINT/ping/v1/$TELMONI_PING_KEY/nightly-report"',
      'curl -fsS -m 10 "$TELMONI_ENDPOINT/ping/v1/$TELMONI_PING_KEY/nightly-report/fail"',
      "telmoni run --monitor nightly-report -- <command>",
      'with telmoni.run("report-bot"):',
    ]);
  });
});

describe("a monitor's page", () => {
  const monitor = { stall_after_s: 600, max_steps: null, max_cost_usd: 2 };

  it("says each bound is the monitor's while no run is going", () => {
    expect(boundSources(monitor).map((b) => [b.key, b.held, b.from])).toEqual([
      ["stall_after_s", 600, "monitor"],
      ["max_steps", null, null],
      ["max_cost_usd", 2, "monitor"],
    ]);
  });

  it("says which bounds a running run's own code made tighter", () => {
    const running = { stall_after_s: 60, max_steps: 40, max_cost_usd: 2 };
    expect(boundSources(monitor, running).map((b) => [b.key, b.held, b.from])).toEqual([
      ["stall_after_s", 60, "code"],
      ["max_steps", 40, "code"],
      ["max_cost_usd", 2, "monitor"],
    ]);
  });

  it("words each of its recorded changes", () => {
    expect(changeLine({ action: "created", metadata: { slug: "a", agent: "a" } })).toBe("Created");
    expect(
      changeLine({ action: "updated", metadata: { changed: ["schedule", "grace_s", "max_steps"] } }),
    ).toBe("Changed its schedule, grace period and step cap");
    expect(changeLine({ action: "updated", metadata: { state: { from: "healthy", to: "paused" } } })).toBe(
      "Paused",
    );
    expect(changeLine({ action: "updated", metadata: { state: { from: "paused", to: "healthy" } } })).toBe(
      "Resumed",
    );
    expect(
      changeLine({
        action: "updated",
        metadata: { changed: ["schedule"], state: { from: "new", to: "healthy" } },
      }),
    ).toBe("Set up: its schedule");
    expect(changeLine({ action: "updated", metadata: null })).toBe("Changed");
  });

  it("says how long a resolved incident stood", () => {
    expect(stood({ opened_at: "2026-10-21T02:00:00Z", resolved_at: "2026-10-21T03:04:00Z" })).toBe(
      "1 h 4 min",
    );
  });
});
