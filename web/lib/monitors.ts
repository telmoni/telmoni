import { listHref, many, one, type SearchParams } from "@/lib/list-query";
import type { Bounds, Monitor } from "@/lib/server/entities/monitors";
import { projectPath } from "@/lib/slug";
import { STATUS_LABEL, formatCount, formatDuration } from "@/lib/traces";
import { IncidentDetail, IncidentResolution, MonitorState } from "@/lib/types/enums";

/** The word the console gives each state. */
export const STATE_LABEL: Record<MonitorState, string> = {
  [MonitorState.Failed]: "Failed",
  [MonitorState.Stalled]: "Stalled",
  [MonitorState.OverBound]: "Over a bound",
  [MonitorState.Late]: "Late",
  [MonitorState.Running]: "Running",
  [MonitorState.New]: "New",
  [MonitorState.Paused]: "Paused",
  [MonitorState.Healthy]: "Healthy",
};

/** Every state, the worst first: the order the board and the Monitors page
 *  sort in, held to the contract's by `contract.test.ts`. */
export const MONITOR_STATES: readonly MonitorState[] = Object.values(MonitorState);

/** The states that need someone: a run failed, gone quiet or past a bound,
 *  or a run expected that has not come. */
export const NEEDS_SOMEONE: readonly MonitorState[] = [
  MonitorState.Failed,
  MonitorState.Stalled,
  MonitorState.OverBound,
  MonitorState.Late,
];

export type Counted = { state: MonitorState; count: number };

export type Board<M extends { state: MonitorState }> = {
  /** The monitors that need someone, in the order given: the worst first. */
  needing: M[];
  /** Those with a run going. */
  running: M[];
  /** Every other monitor, counted by state in the contract's order; a state
   *  with none is left out. */
  counted: Counted[];
};

/** The board of `monitors`, in the order the lane answers them: the worst
 *  state first, then by name. */
export function boardOf<M extends { state: MonitorState }>(monitors: readonly M[]): Board<M> {
  const needing = monitors.filter((m) => NEEDS_SOMEONE.includes(m.state));
  const running = monitors.filter((m) => m.state === MonitorState.Running);
  const counted = MONITOR_STATES.filter(
    (state) => !NEEDS_SOMEONE.includes(state) && state !== MonitorState.Running,
  )
    .map((state) => ({ state, count: monitors.filter((m) => m.state === state).length }))
    .filter(({ count }) => count > 0);
  return { needing, running, counted };
}

/** The rest of the board in one line: `12 healthy`, `2 new · 12 healthy`. */
export function countedLine(counted: readonly Counted[]): string {
  return counted
    .map(({ state, count }) => `${formatCount(count)} ${STATE_LABEL[state].toLowerCase()}`)
    .join(" · ");
}

// Dollars to the cent, and below a cent to the hundredth of one, since a
// run's spend is often a fraction of a cent; in the console's own language,
// as `formatCount` is, so the server and the browser draw the same text.
const SPEND = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 4,
});

/** Spend in US dollars: `$0.08`, `$0.0012`, `$1,234.50`. */
export function formatSpend(usd: number): string {
  return SPEND.format(usd);
}

/** A monitor's last run in a line, after its time: how it ended, how long it
 *  took where it had a start, and its steps and spend where it reported any,
 *  each against the monitor's cap where one is set — `OK · 4 min 2 s · 12 /
 *  40 steps · $0.08 / $2.00`; `null` where no run has ended. */
export function lastRunLine(monitor: Pick<Monitor, "last_run" | "bounds">): string | null {
  const run = monitor.last_run;
  if (!run) return null;
  const parts = [STATUS_LABEL[run.status]];
  const ms = Date.parse(run.ended_at) - Date.parse(run.started_at);
  // A run of no duration is a bare ping, whose length nothing knows.
  if (Number.isFinite(ms) && ms > 0) parts.push(formatDuration(ms));
  if (run.steps !== null) {
    const cap = monitor.bounds.max_steps;
    const steps = cap === null ? formatCount(run.steps) : `${formatCount(run.steps)} / ${formatCount(cap)}`;
    parts.push(`${steps} ${run.steps === 1 && cap === null ? "step" : "steps"}`);
  }
  if (run.cost_usd !== null) {
    const cap = monitor.bounds.max_cost_usd;
    parts.push(
      cap === null
        ? formatSpend(run.cost_usd)
        : `${formatSpend(run.cost_usd)} / ${formatSpend(cap)}`,
    );
  }
  return parts.join(" · ");
}

// ── Words ───────────────────────────────────────────────────────────────────

const DAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const DAY_NAMES = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/** How often, from a period in whole seconds: `every 15 minutes`, `every
 *  hour`, `every 2 days`, `every 90 seconds`. */
export function everyPeriod(seconds: number): string {
  const [count, unit] =
    seconds % 86_400 === 0
      ? [seconds / 86_400, "day"]
      : seconds % 3_600 === 0
        ? [seconds / 3_600, "hour"]
        : seconds % 60 === 0
          ? [seconds / 60, "minute"]
          : [seconds, "second"];
  return count === 1 ? `every ${unit}` : `every ${formatCount(count)} ${unit}s`;
}

/** `09:05` from a cron field's hour and minute, or `null` where either is not
 *  one plain number in range. */
function clock(hour: string, minute: string): string | null {
  if (!/^\d{1,2}$/.test(hour) || !/^\d{1,2}$/.test(minute)) return null;
  const [h, m] = [Number(hour), Number(minute)];
  if (h > 23 || m > 59) return null;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}`;
}

/** A day-of-week field's days, Sunday first, or `null` for one it does not
 *  read plainly: a list of single days and ranges, by number or by name. */
function weekdays(field: string): number[] | null {
  const day = (word: string): number | null => {
    if (/^[0-7]$/.test(word)) return Number(word) % 7;
    const at = DAY_NAMES.indexOf(word.toLowerCase());
    return at === -1 ? null : at;
  };
  const days = new Set<number>();
  for (const part of field.split(",")) {
    const [from, to, ...rest] = part.split("-");
    if (from === undefined || rest.length > 0) return null;
    const start = day(from);
    const end = to === undefined ? start : day(to);
    if (start === null || end === null || end < start) return null;
    for (let d = start; d <= end; d++) days.add(d);
  }
  return [...days].sort((a, b) => a - b);
}

/** Days in a sentence: `Monday`, `weekdays`, `Monday, Wednesday and Friday`. */
function daysInWords(days: readonly number[]): string {
  if (days.join() === "1,2,3,4,5") return "weekday";
  if (days.join() === "0,6") return "Saturday and Sunday";
  const names = days.map((d) => DAYS[d] ?? "");
  return names.length < 2
    ? names.join("")
    : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** The ordinal a person writes a day of the month with: `1st`, `22nd`. */
function ordinal(day: number): string {
  const tens = day % 100;
  const suffix =
    tens >= 11 && tens <= 13 ? "th" : (["th", "st", "nd", "rd"][day % 10] ?? "th");
  return `${day}${suffix}`;
}

const NICKNAMES: Record<string, string> = {
  "@yearly": "every year on 1 January at 00:00",
  "@annually": "every year on 1 January at 00:00",
  "@monthly": "on the 1st of every month at 00:00",
  "@weekly": "every Sunday at 00:00",
  "@daily": "every day at 00:00",
  "@midnight": "every day at 00:00",
  "@hourly": "every hour",
};

/** A five-field cron line in words, for the lines a crontab most often holds;
 *  `null` for any other, which the page then shows as it is. */
export function cronInWords(line: string): string | null {
  const trimmed = line.trim();
  const nickname = NICKNAMES[trimmed.toLowerCase()];
  if (nickname) return nickname;
  const fields = trimmed.split(/\s+/);
  if (fields.length !== 5) return null;
  const [minute, hour, dom, month, dow] = fields as [string, string, string, string, string];
  if (month !== "*") return null;
  const step = /^\*\/(\d{1,2})$/;
  if (dom === "*" && dow === "*") {
    if (hour === "*" && minute === "*") return "every minute";
    const everyMinutes = step.exec(minute);
    if (hour === "*" && everyMinutes) {
      const n = Number(everyMinutes[1]);
      return n === 1 ? "every minute" : n > 0 && n < 60 ? `every ${n} minutes` : null;
    }
    if (/^\d{1,2}$/.test(minute) && Number(minute) < 60) {
      const past = Number(minute) === 0 ? "" : ` at :${minute.padStart(2, "0")}`;
      if (hour === "*") return `every hour${past}`;
      const everyHours = step.exec(hour);
      if (everyHours) {
        const n = Number(everyHours[1]);
        if (n === 1) return `every hour${past}`;
        if (n > 0 && n < 24) return `every ${n} hours${past}`;
        return null;
      }
    }
  }
  const at = clock(hour, minute);
  if (at === null) return null;
  if (dom === "*" && dow === "*") return `every day at ${at}`;
  if (dom === "*") {
    const days = weekdays(dow);
    if (days === null || days.length === 0) return null;
    if (days.length === 7) return `every day at ${at}`;
    return `every ${daysInWords(days)} at ${at}`;
  }
  if (dow === "*" && /^\d{1,2}$/.test(dom) && Number(dom) >= 1 && Number(dom) <= 31) {
    return `on the ${ordinal(Number(dom))} of every month at ${at}`;
  }
  return null;
}

/** When a monitor expects its runs, in words: `every day at 02:00,
 *  Europe/London`, `every 15 minutes`, `no schedule`; a cron line it cannot
 *  word is given as it is, with its zone. */
export function scheduleInWords(
  monitor: Pick<Monitor, "schedule" | "interval_s" | "timezone">,
): string {
  if (monitor.interval_s !== null) return everyPeriod(monitor.interval_s);
  if (monitor.schedule === null) return "no schedule";
  return `${cronInWords(monitor.schedule) ?? monitor.schedule}, ${monitor.timezone}`;
}

/** Whole seconds as a duration reads: `10 min`, `1 h 30 min`, `45 s`. */
export function secondsInWords(seconds: number): string {
  return formatDuration(seconds * 1_000);
}

/** A monitor's bounds in brief, `stall 10 min · 40 steps · $2.00`; `null` for
 *  one held to none. */
export function boundsInBrief(bounds: Bounds): string | null {
  const parts: string[] = [];
  if (bounds.stall_after_s !== null) parts.push(`stall ${secondsInWords(bounds.stall_after_s)}`);
  if (bounds.max_steps !== null) {
    parts.push(`${formatCount(bounds.max_steps)} ${bounds.max_steps === 1 ? "step" : "steps"}`);
  }
  if (bounds.max_cost_usd !== null) parts.push(formatSpend(bounds.max_cost_usd));
  return parts.length > 0 ? parts.join(" · ") : null;
}

/** What crossed, as an incident's line says it. */
export const DETAIL_LABEL: Record<IncidentDetail, string> = {
  [IncidentDetail.RunFailed]: "A run ended in error",
  [IncidentDetail.RunNeverCame]: "A run never came within its grace",
  [IncidentDetail.RunNeverEnded]: "A run never said it ended",
  [IncidentDetail.RunLate]: "A run was late",
  [IncidentDetail.RunStalled]: "A run went quiet past its stall timeout",
  [IncidentDetail.RunOverBound]: "A run crossed its step or cost cap",
};

/** How an incident closed. */
export const RESOLUTION_LABEL: Record<IncidentResolution, string> = {
  [IncidentResolution.Recovered]: "Recovered",
  [IncidentResolution.Paused]: "Paused",
};

// ── The list's query ────────────────────────────────────────────────────────

/** What the Monitors page narrows its list to, from its URL: the states
 *  picked, and words its names hold. */
export type MonitorQuery = { state?: MonitorState[]; q?: string };

/** The longest search the page takes, as a monitor's name is at most 60. */
const SEARCH_MAX = 60;

export function parseMonitorQuery(searchParams: SearchParams): MonitorQuery {
  const state = many(searchParams.state).filter((s): s is MonitorState =>
    (MONITOR_STATES as readonly string[]).includes(s),
  );
  const q = one(searchParams.q);
  return {
    ...(state.length > 0 ? { state } : {}),
    ...(q !== undefined ? { q: [...q].slice(0, SEARCH_MAX).join("") } : {}),
  };
}

/** The monitors `query` admits, in the order given. */
export function filterMonitors<M extends Pick<Monitor, "name" | "state">>(
  monitors: readonly M[],
  query: MonitorQuery,
): M[] {
  const words = query.q?.toLowerCase();
  return monitors.filter(
    (m) =>
      (query.state === undefined || query.state.includes(m.state)) &&
      (words === undefined || m.name.toLowerCase().includes(words)),
  );
}

/** The Monitors page's path with `query` in its URL. */
export function monitorsHref(
  organization: string,
  project: string,
  query: MonitorQuery,
  patch: Partial<Record<keyof MonitorQuery, string | readonly string[] | undefined>> = {},
): string {
  return listHref(organization, project, "/monitors", query, patch);
}

/** A monitor's own page. */
export function monitorHref(organization: string, project: string, slug: string): string {
  return projectPath(organization, project, `/monitors/${slug}`);
}

// ── The form ────────────────────────────────────────────────────────────────

/** How a monitor's runs are expected: not at all, every so often, or on a
 *  cron line. */
export type Timing = "none" | "interval" | "cron";

/** The units a duration is typed in. */
export type Unit = "seconds" | "minutes" | "hours";

const UNIT_SECONDS: Record<Unit, number> = { seconds: 1, minutes: 60, hours: 3_600 };

/** A duration as it is typed: a number and its unit. */
export type Typed = { value: string; unit: Unit };

/** The monitor form's fields, as typed. */
export type MonitorForm = {
  name: string;
  slug: string;
  agent: string;
  timing: Timing;
  every: Typed;
  cron: string;
  timezone: string;
  grace: Typed;
  expected: Typed;
  stall: Typed;
  steps: string;
  /** The cost cap the monitor holds, kept as it is: the form sets none until
   *  spend is priced, and must not drop one the API set. */
  cost: number | null;
};

/** The duration a whole number of seconds is typed as: its largest unit that
 *  divides it, or blank for none. */
export function typedOf(seconds: number | null, fallback: Unit = "minutes"): Typed {
  if (seconds === null) return { value: "", unit: fallback };
  if (seconds === 0) return { value: "0", unit: fallback };
  const unit = (["hours", "minutes", "seconds"] as const).find(
    (u) => seconds % UNIT_SECONDS[u] === 0,
  ) ?? "seconds";
  return { value: String(seconds / UNIT_SECONDS[unit]), unit };
}

/** A new monitor's form: daily-ish defaults a person changes, in their zone. */
export function blankForm(timezone: string): MonitorForm {
  return {
    name: "",
    slug: "",
    agent: "",
    timing: "cron",
    every: { value: "15", unit: "minutes" },
    cron: "0 2 * * *",
    timezone,
    grace: { value: "10", unit: "minutes" },
    expected: { value: "", unit: "minutes" },
    stall: { value: "", unit: "minutes" },
    steps: "",
    cost: null,
  };
}

/** A monitor as its form edits it. */
export function formOf(monitor: Monitor): MonitorForm {
  return {
    name: monitor.name,
    slug: monitor.slug,
    agent: monitor.agent,
    timing: monitor.interval_s !== null ? "interval" : monitor.schedule !== null ? "cron" : "none",
    every: typedOf(monitor.interval_s),
    cron: monitor.schedule ?? "",
    timezone: monitor.timezone,
    grace: typedOf(monitor.grace_s),
    expected: typedOf(monitor.expected_duration_s),
    stall: typedOf(monitor.bounds.stall_after_s),
    steps: monitor.bounds.max_steps === null ? "" : String(monitor.bounds.max_steps),
    cost: monitor.bounds.max_cost_usd,
  };
}

/** The slug a name reads as, as the server makes one: Latin letters without
 *  their accents and digits, a hyphen wherever anything else stood, cut to 48
 *  at a word's end. Empty when nothing survives. */
export function slugFromName(name: string): string {
  const words = name
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase()
    .replace(/['’]/g, "")
    .split(/[^a-z0-9]+/)
    .filter(Boolean);
  let slug = "";
  for (const word of words) {
    const next = slug ? `${slug}-${word}` : word;
    if (next.length > 48) return slug || word.slice(0, 48);
    slug = next;
  }
  return slug;
}

/** What the lane is sent, or the field a person must fix first and why. */
export type Checked =
  | { ok: true; body: MonitorBody }
  | { ok: false; field: keyof MonitorForm; message: string };

/** A monitor as `POST` and `PUT /internal/v1/telemetry/monitors` take it. */
export type MonitorBody = {
  name: string;
  slug?: string;
  agent?: string;
  schedule?: string;
  interval_s?: number;
  timezone?: string;
  grace_s?: number;
  expected_duration_s?: number;
  bounds: { stall_after_s?: number; max_steps?: number; max_cost_usd?: number };
};

/** Whole seconds from a typed duration, `undefined` for one left blank, or
 *  `null` for one that is no positive whole number of seconds. */
function seconds(typed: Typed, zeroAllowed = false): number | undefined | null {
  const text = typed.value.trim();
  if (text === "") return undefined;
  if (!/^\d+(\.\d+)?$/.test(text)) return null;
  const total = Number(text) * UNIT_SECONDS[typed.unit];
  if (!Number.isInteger(total) || (total === 0 && !zeroAllowed)) return null;
  return total;
}

/** The form checked as the page can check it, the lane's own rules the
 *  lane's to answer: what it sends, or the first field to fix. */
export function checkForm(form: MonitorForm): Checked {
  const fail = (field: keyof MonitorForm, message: string): Checked => ({
    ok: false,
    field,
    message,
  });
  const name = form.name.trim();
  if (!name) return fail("name", "Name the monitor.");
  const slug = form.slug.trim();
  if (!slug) return fail("slug", "Give it a slug: lowercase letters and digits, joined by hyphens.");
  const body: MonitorBody = { name, slug, bounds: {} };
  const agent = form.agent.trim();
  if (agent && agent !== slug) body.agent = agent;
  if (form.timing === "interval") {
    const every = seconds(form.every);
    if (every === undefined || every === null) {
      return fail("every", "Say how often it runs, in whole seconds or more.");
    }
    body.interval_s = every;
  } else if (form.timing === "cron") {
    const line = form.cron.trim();
    if (!line) return fail("cron", "Write the cron line its crontab holds.");
    body.schedule = line;
    body.timezone = form.timezone;
  }
  if (form.timing !== "none") {
    const grace = seconds(form.grace, true);
    if (grace === undefined || grace === null) {
      return fail("grace", "Say how long past its time a run may start, 0 or more.");
    }
    body.grace_s = grace;
  }
  const expected = seconds(form.expected);
  if (expected === null) return fail("expected", "An expected duration is a positive length.");
  if (expected !== undefined) body.expected_duration_s = expected;
  const stall = seconds(form.stall);
  if (stall === null) return fail("stall", "A stall timeout is a positive length.");
  if (stall !== undefined) body.bounds.stall_after_s = stall;
  const steps = form.steps.trim();
  if (steps) {
    if (!/^\d+$/.test(steps) || Number(steps) < 1) {
      return fail("steps", "A step cap is a whole number, 1 or more.");
    }
    body.bounds.max_steps = Number(steps);
  }
  if (form.cost !== null) body.bounds.max_cost_usd = form.cost;
  return { ok: true, body };
}

// ── Reporting a monitor's runs ──────────────────────────────────────────────

/** One way to report a monitor's runs: what it is for, and the line to copy. */
export type ReportLine = { label: string; line: string };

/** The ways a monitor's runs are reported, each a line to copy: the two
 *  pings a crontab's `curl` sends, the CLI wrapping the job, and an SDK's run
 *  naming the agent. The key is the variable, never the key: it is kept
 *  hashed, and shown once, when minted. */
export function reportLines(slug: string, agent: string): ReportLine[] {
  const ping = (tail: string) =>
    `curl -fsS -m 10 "$TELMONI_ENDPOINT/ping/v1/$TELMONI_PING_KEY/${slug}${tail}"`;
  return [
    { label: "As the job starts", line: ping("/start") },
    { label: "As it ends well", line: ping("") },
    { label: "Or as it fails", line: ping("/fail") },
    { label: "Or wrap the job", line: `telmoni run --monitor ${slug} -- <command>` },
    { label: "Or from its code", line: `with telmoni.run("${agent}"):` },
  ];
}

// ── One monitor's page ──────────────────────────────────────────────────────

/** Which bound a source sets, by its field on `Bounds`. */
export type BoundKey = keyof Bounds;

/** One bound as the page explains it: what the monitor sets, what a run is
 *  held to, and whether that is the monitor's or the run's own code's. */
export type BoundSource = {
  key: BoundKey;
  monitor: number | null;
  held: number | null;
  from: "monitor" | "code" | null;
};

/** Each bound of `monitor`, and, where a run is going, what it is held to:
 *  the tighter of the monitor's and its code's, so a value tighter than the
 *  monitor's, or one where it sets none, is the code's. */
export function boundSources(monitor: Bounds, running?: Bounds): BoundSource[] {
  return (["stall_after_s", "max_steps", "max_cost_usd"] as const).map((key) => {
    const own = monitor[key];
    const held = running ? running[key] : own;
    const from =
      held === null ? null : !running || (own !== null && held >= own) ? "monitor" : "code";
    return { key, monitor: own, held, from };
  });
}

/** A bound's value as a person reads it. */
export function boundValue(key: BoundKey, value: number): string {
  if (key === "stall_after_s") return secondsInWords(value);
  if (key === "max_steps") return `${formatCount(value)} ${value === 1 ? "step" : "steps"}`;
  return formatSpend(value);
}

/** The words a monitor's fields go by in its recent changes. */
const FIELD_WORDS: Record<string, string> = {
  name: "name",
  agent: "agent",
  schedule: "schedule",
  interval_s: "interval",
  timezone: "timezone",
  grace_s: "grace period",
  expected_duration_s: "expected duration",
  stall_after_s: "stall timeout",
  max_steps: "step cap",
  max_cost_usd: "cost cap",
};

/** A list in a sentence: `name`, `name and agent`, `name, agent and schedule`. */
function listed(words: readonly string[]): string {
  return words.length < 2 ? words.join("") : `${words.slice(0, -1).join(", ")} and ${words.at(-1)}`;
}

/** What one of a monitor's audit rows did, in words: `Created`, `Paused`,
 *  `Resumed`, `Set up`, `Changed its schedule and grace period`. */
export function changeLine(event: { action: string; metadata?: unknown }): string {
  const metadata =
    typeof event.metadata === "object" && event.metadata !== null
      ? (event.metadata as Record<string, unknown>)
      : {};
  if (event.action === "created") return "Created";
  if (event.action === "deleted") return "Deleted";
  const state = metadata.state as { from?: string; to?: string } | undefined;
  const changed = Array.isArray(metadata.changed)
    ? metadata.changed.filter((f): f is string => typeof f === "string")
    : [];
  const fields = listed(changed.map((f) => FIELD_WORDS[f] ?? f));
  if (state?.from === MonitorState.New) return fields ? `Set up: its ${fields}` : "Set up";
  if (state?.to === MonitorState.Paused) return "Paused";
  if (state?.from === MonitorState.Paused) return "Resumed";
  return fields ? `Changed its ${fields}` : "Changed";
}

/** How long a resolved incident stood. */
export function stood(incident: { opened_at: string; resolved_at: string }): string {
  return formatDuration(
    Math.max(0, Date.parse(incident.resolved_at) - Date.parse(incident.opened_at)),
  );
}
