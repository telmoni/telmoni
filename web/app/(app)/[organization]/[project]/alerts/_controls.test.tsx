// @vitest-environment jsdom
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { Answer as Act } from "./_act";

vi.mock("next/navigation", () => ({
  unstable_rethrow: () => {},
}));

// No console shell in a test: the panel is drawn in place.
vi.mock("@/components/console-shell", () => ({
  useConsoleShell: () => ({ element: null, cover: () => () => {} }),
}));
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}));

type Answer = Promise<Act>;
const saveAlertAction = vi.fn<(p: string, r: string | null, i: unknown) => Answer>(async () => ({ ok: true }));
const pauseAlertAction = vi.fn<(p: string, r: string, paused: boolean) => Answer>(async () => ({ ok: true }));
const deleteAlertAction = vi.fn<(p: string, r: string) => Answer>(async () => ({ ok: true }));
const hearAgainAction = vi.fn<(p: string, r: string, c: string) => Answer>(async () => ({ ok: true }));
const incidentAction = vi.fn<(p: string, i: string, act: string) => Answer>(async () => ({ ok: true }));
const muteAction = vi.fn<(p: string, i: unknown) => Answer>(async () => ({ ok: true }));
const endMuteAction = vi.fn<(p: string, m: string) => Answer>(async () => ({ ok: true }));
vi.mock("./actions", () => ({
  saveAlertAction: (p: string, r: string | null, i: unknown) => saveAlertAction(p, r, i),
  pauseAlertAction: (p: string, r: string, paused: boolean) => pauseAlertAction(p, r, paused),
  deleteAlertAction: (p: string, r: string) => deleteAlertAction(p, r),
  hearAgainAction: (p: string, r: string, c: string) => hearAgainAction(p, r, c),
  incidentAction: (p: string, i: string, act: string) => incidentAction(p, i, act),
  muteAction: (p: string, i: unknown) => muteAction(p, i),
  endMuteAction: (p: string, m: string) => endMuteAction(p, m),
}));

import { toast } from "sonner";

import { ALERTS_HEADING, alertHeading, type ConnectionChoice } from "@/lib/alerts";
import type { Alert } from "@/lib/server/data";

import { AlertActions, HearAgain } from "./_alert-actions";
import { NewAlert } from "./_alert-panel";
import { IncidentActions } from "./_incident-actions";
import { EndMute, MuteButton } from "./_mute";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const RULE = "7a1c0e52-0000-4000-8000-000000000002";
const SLACK = "3c4e2a74-0000-4000-8000-000000000004";
const HOOK = "4d5f3b85-0000-4000-8000-000000000007";
const CONNECTIONS: ConnectionChoice[] = [
  { id: SLACK, name: "#alerts in Acme", status: "active", eventKinds: null },
  { id: HOOK, name: "hooks.example.com", status: "errored", eventKinds: null },
];

/** A choice made in one of the console's selects, through the native select
 *  it keeps beside its trigger for the form. */
function pick(trigger: HTMLElement, value: string) {
  const native = trigger.nextElementSibling;
  if (!(native instanceof HTMLSelectElement)) throw new Error("no native select beside the trigger");
  fireEvent.change(native, { target: { value } });
}

const ALERT: Alert = {
  id: RULE,
  name: "Slow calls",
  kind: "model_call",
  measure: "duration",
  aggregation: "p95",
  filters: { model: ["claude-sonnet-5-5"] },
  window: "hour",
  operator: "gt",
  alert_threshold: 5000,
  warning_threshold: 3000,
  missing_data: "zero",
  renotify_minutes: null,
  paused: null,
  paused_at: null,
  value: null,
  evaluated_at: null,
  severity: "unknown",
  severity_since: "2026-10-26T11:00:00Z",
  connectors: [{ connection_id: SLACK, level: "alert", hours: null, muted: false }],
  created_at: "2026-10-25T00:00:00Z",
  updated_at: "2026-10-25T00:00:00Z",
};

beforeEach(() => {
  vi.clearAllMocks();
  // A menu's popper measures its trigger, which jsdom cannot.
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});

// The form draws a dozen of the console's controls, which the whole suite's
// load can hold past the default five seconds.
describe("the alert's form", { timeout: 15_000 }, () => {
  it("opens a new alert in the side panel, full over the content area, the name focused", () => {
    render(<NewAlert projectId={PROJECT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    expect(panel).toHaveAttribute("data-mode", "full");
    expect(within(panel).getByRole("heading", { name: "New alert" })).toBeInTheDocument();
    expect(document.activeElement).toBe(within(panel).getByLabelText("Name"));
    // The share of runs that failed over the last hour, its thresholds to type.
    expect(within(panel).getByRole("combobox", { name: "Reads" })).toHaveTextContent("Runs");
    expect(within(panel).getByRole("combobox", { name: "Measure" })).toHaveTextContent("Failures");
    expect(within(panel).getByRole("combobox", { name: "Folded as" })).toHaveTextContent("Share failed");
    // Both thresholds are typed as percentages.
    expect(within(panel).getAllByText("%", { selector: "span" })).toHaveLength(2);
    fireEvent.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(saveAlertAction).not.toHaveBeenCalled();
  });

  it("makes the alert as filled in, a share sent as one, and shuts the panel", async () => {
    render(<NewAlert projectId={PROJECT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "Failing trips" } });
    fireEvent.change(within(panel).getByLabelText("Agent"), { target: { value: "trip, triage" } });
    fireEvent.click(within(panel).getByRole("checkbox", { name: "Error" }));
    fireEvent.change(within(panel).getByLabelText("Alert when"), { target: { value: "10" } });
    fireEvent.change(within(panel).getByLabelText("Warn when"), { target: { value: "5" } });
    fireEvent.change(within(panel).getByLabelText("Notify again"), { target: { value: "60" } });
    fireEvent.click(within(panel).getByRole("switch", { name: "#alerts in Acme" }));
    fireEvent.click(within(panel).getByRole("button", { name: "Make alert" }));
    await waitFor(() => expect(saveAlertAction).toHaveBeenCalledOnce());
    expect(saveAlertAction).toHaveBeenCalledWith(PROJECT, null, {
      name: "Failing trips",
      kind: "run",
      measure: "failures",
      aggregation: "avg",
      filters: { agent: ["trip", "triage"], status: ["error"] },
      window: "hour",
      operator: "gt",
      alert_threshold: 0.1,
      warning_threshold: 0.05,
      missing_data: "zero",
      renotify_minutes: 60,
      connectors: [{ connection_id: SLACK, level: "warning", hours: null }],
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("says what is wrong beside the form and sends nothing, and keeps the lane's refusal", async () => {
    render(<NewAlert projectId={PROJECT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    fireEvent.click(within(panel).getByRole("button", { name: "Make alert" }));
    expect(within(panel).getByRole("alert")).toHaveTextContent("Name the alert");
    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "Failing trips" } });
    fireEvent.change(within(panel).getByLabelText("Alert when"), { target: { value: "10" } });
    fireEvent.change(within(panel).getByLabelText("Warn when"), { target: { value: "20" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Make alert" }));
    expect(within(panel).getByRole("alert")).toHaveTextContent("below the alert threshold");
    expect(saveAlertAction).not.toHaveBeenCalled();

    saveAlertAction.mockResolvedValueOnce({ ok: false, error: "this project has an alert by that name already" });
    fireEvent.change(within(panel).getByLabelText("Warn when"), { target: { value: "" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Make alert" }));
    expect(await within(panel).findByRole("alert")).toHaveTextContent("an alert by that name already");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("offers each connector, one that needs attention marked, and links it at its set hours", async () => {
    // The browser's clock an hour east of UTC.
    const clock = vi.spyOn(Date.prototype, "getTimezoneOffset").mockReturnValue(-60);
    render(<NewAlert projectId={PROJECT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    expect(within(panel).getByText("Needs attention")).toBeInTheDocument();
    fireEvent.click(within(panel).getByRole("switch", { name: "hooks.example.com" }));
    expect(within(panel).getByRole("combobox", { name: "Hears" })).toHaveTextContent("Warnings and alerts");
    expect(within(panel).getByRole("combobox", { name: "Hears" })).toHaveAccessibleDescription(
      "A no-data incident reaches only the connectors that hear warnings and alerts.",
    );
    const when = within(panel).getByRole("combobox", { name: "When" });
    expect(when).toHaveTextContent("At any hour");

    pick(when, "set");
    const days = within(panel).getByRole("toolbar", { name: "Days" });
    const pressed = () =>
      within(days)
        .getAllByRole("button")
        .filter((day) => day.getAttribute("aria-pressed") === "true")
        .map((day) => day.textContent);
    expect(pressed()).toEqual(["Mon", "Tue", "Wed", "Thu", "Fri"]);
    expect(within(panel).getByLabelText("From")).toHaveValue("09:00");
    expect(within(panel).getByLabelText("to")).toHaveValue("18:00");
    const offset = within(panel).getByRole("combobox", { name: "Offset" });
    expect(offset).toHaveTextContent("UTC+01:00");
    fireEvent.click(within(days).getByRole("button", { name: "Sat" }));
    fireEvent.click(within(days).getByRole("button", { name: "Fri" }));
    fireEvent.change(within(panel).getByLabelText("From"), { target: { value: "08:30" } });
    pick(offset, "330");

    fireEvent.change(within(panel).getByLabelText("Name"), { target: { value: "Failing trips" } });
    fireEvent.change(within(panel).getByLabelText("Alert when"), { target: { value: "10" } });
    fireEvent.click(within(panel).getByRole("button", { name: "Make alert" }));
    await waitFor(() => expect(saveAlertAction).toHaveBeenCalledOnce());
    expect(saveAlertAction.mock.calls[0]?.[2]).toMatchObject({
      connectors: [
        {
          connection_id: HOOK,
          level: "warning",
          hours: { days: [1, 2, 3, 4, 6], from: "08:30", to: "18:00", utc_offset: 330 },
        },
      ],
    });
    clock.mockRestore();
  });

  // ⚠ What voice control is told to press is what the screen shows.
  it("names each of its selects by a label on screen", () => {
    render(<NewAlert projectId={PROJECT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    fireEvent.click(within(panel).getByRole("switch", { name: "hooks.example.com" }));
    pick(within(panel).getByRole("combobox", { name: "When" }), "set");
    const selects = within(panel).getAllByRole("combobox");
    expect(selects).toHaveLength(9);
    for (const select of selects) {
      expect(select).not.toHaveAttribute("aria-label");
      expect(panel.querySelector(`label[for="${select.id}"]`), select.textContent ?? "").not.toBeNull();
    }
    expect(within(panel).getByRole("combobox", { name: "Compared as" })).toHaveTextContent("Above (>)");
  });

  // ⚠ Notifications sends a webhook only the kinds it takes, an alert's incidents among them.
  it("marks a webhook that takes neither of an incident's kinds, or one alone", () => {
    const webhook = (id: string, eventKinds: ConnectionChoice["eventKinds"]): ConnectionChoice => ({
      id,
      name: `${id}.example.com`,
      status: "active",
      eventKinds,
    });
    render(
      <NewAlert
        projectId={PROJECT}
        connections={[
          ...CONNECTIONS,
          webhook("none", ["member_added"]),
          webhook("opened", ["incident_opened"]),
          webhook("resolved", ["member_left", "incident_resolved"]),
          webhook("both", ["incident_resolved", "incident_opened"]),
        ]}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    const panel = screen.getByRole("dialog");
    const row = (name: string) => within(panel).getByRole("switch", { name }).closest("li")!;
    expect(row("none.example.com")).toHaveTextContent("Takes no incidents");
    expect(row("opened.example.com")).toHaveTextContent("Takes openings only");
    expect(row("resolved.example.com")).toHaveTextContent("Takes resolutions only");
    for (const name of ["both.example.com", "#alerts in Acme"]) {
      expect(row(name)).not.toHaveTextContent(/Takes/);
    }
  });

  it("says the alert's incidents reach the feed alone where the project connects nothing", () => {
    render(<NewAlert projectId={PROJECT} connections={[]} />);
    fireEvent.click(screen.getByRole("button", { name: /New alert/ }));
    expect(screen.getByText(/reach the project.s feed alone/)).toBeInTheDocument();
  });
});

describe("an alert's controls", { timeout: 15_000 }, () => {
  it("edits the alert in the panel as it stands, and saves it over by its id", async () => {
    render(<AlertActions projectId={PROJECT} alert={ALERT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: "Edit Slow calls" }));
    const panel = screen.getByRole("dialog");
    expect(within(panel).getByRole("heading", { name: "Edit Slow calls" })).toBeInTheDocument();
    expect(within(panel).getByLabelText("Name")).toHaveValue("Slow calls");
    expect(within(panel).getByLabelText("Model")).toHaveValue("claude-sonnet-5-5");
    expect(within(panel).getByLabelText("Alert when")).toHaveValue("5000");
    expect(within(panel).getByRole("switch", { name: "#alerts in Acme" })).toBeChecked();
    expect(within(panel).getByRole("combobox", { name: "Hears" })).toHaveTextContent("Alerts only");
    fireEvent.click(within(panel).getByRole("button", { name: "Save alert" }));
    await waitFor(() => expect(saveAlertAction).toHaveBeenCalledOnce());
    expect(saveAlertAction).toHaveBeenCalledWith(
      PROJECT,
      RULE,
      expect.objectContaining({
        name: "Slow calls",
        filters: { model: ["claude-sonnet-5-5"] },
        alert_threshold: 5000,
        warning_threshold: 3000,
        connectors: [{ connection_id: SLACK, level: "alert", hours: null }],
      }),
    );
  });

  it("pauses a running alert, resumes a paused one, and deletes one once confirmed", async () => {
    const { unmount } = render(<AlertActions projectId={PROJECT} alert={ALERT} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: "Pause Slow calls" }));
    await waitFor(() => expect(pauseAlertAction).toHaveBeenCalledWith(PROJECT, RULE, true));
    // The row's controls wait out the pause before another is taken.
    await waitFor(() => expect(screen.getByRole("button", { name: "Delete Slow calls" })).toBeEnabled());

    fireEvent.click(screen.getByRole("button", { name: "Delete Slow calls" }));
    const confirm = await screen.findByRole("alertdialog");
    expect(deleteAlertAction).not.toHaveBeenCalled();
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(deleteAlertAction).toHaveBeenCalledWith(PROJECT, RULE));
    unmount();

    render(
      <AlertActions projectId={PROJECT} alert={{ ...ALERT, paused: "person" }} connections={CONNECTIONS} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "Resume Slow calls" }));
    await waitFor(() => expect(pauseAlertAction).toHaveBeenCalledWith(PROJECT, RULE, false));
  });

  // ⚠ Room returning would run it again, so a member may keep it off first.
  it("offers to keep an alert the plan paused paused, as the member's own pause", async () => {
    render(<AlertActions projectId={PROJECT} alert={{ ...ALERT, paused: "plan" }} connections={CONNECTIONS} />);
    expect(screen.getByRole("button", { name: "Resume Slow calls" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Keep Slow calls paused" }));
    await waitFor(() => expect(pauseAlertAction).toHaveBeenCalledWith(PROJECT, RULE, true));
  });

  it("offers no second pause on an alert a member paused", () => {
    render(
      <AlertActions projectId={PROJECT} alert={{ ...ALERT, paused: "person" }} connections={CONNECTIONS} />,
    );
    expect(screen.queryByRole("button", { name: "Keep Slow calls paused" })).toBeNull();
  });

  it("says what the lane refused beside the alert", async () => {
    pauseAlertAction.mockResolvedValueOnce({ ok: false, error: "This organization's plan holds 2 alerts." });
    render(<AlertActions projectId={PROJECT} alert={{ ...ALERT, paused: "plan" }} connections={CONNECTIONS} />);
    fireEvent.click(screen.getByRole("button", { name: "Resume Slow calls" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("plan holds 2 alerts");
  });

  it("hears a muted connector again", async () => {
    render(<HearAgain projectId={PROJECT} ruleId={RULE} connectionId={SLACK} name="#alerts in Acme" />);
    fireEvent.click(screen.getByRole("button", { name: "Hear again: #alerts in Acme" }));
    await waitFor(() => expect(hearAgainAction).toHaveBeenCalledWith(PROJECT, RULE, SLACK));
  });

  // ⚠ The focus never falls to the page's body with the control it was on.
  it("hands the focus to the alert's heading, or the section's, once its control goes", async () => {
    const heading = <h3 id={alertHeading(RULE)} tabIndex={-1}>Slow calls</h3>;
    const { rerender } = render(
      <>
        {heading}
        <HearAgain projectId={PROJECT} ruleId={RULE} connectionId={SLACK} name="#alerts in Acme" />
      </>,
    );
    const hear = screen.getByRole("button", { name: "Hear again: #alerts in Acme" });
    hear.focus();
    fireEvent.click(hear);
    await waitFor(() => expect(hear).toBeEnabled());
    expect(hearAgainAction).toHaveBeenCalledOnce();
    // The refresh draws the connector heard again, its button gone.
    rerender(heading);
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Slow calls" }));

    const section = <h2 id={ALERTS_HEADING} tabIndex={-1}>Alerts</h2>;
    rerender(
      <>
        {section}
        <AlertActions projectId={PROJECT} alert={ALERT} connections={CONNECTIONS} />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Delete Slow calls" }));
    const confirm = await screen.findByRole("alertdialog");
    fireEvent.click(within(confirm).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(screen.getByRole("button", { name: "Delete Slow calls" })).toBeEnabled());
    expect(deleteAlertAction).toHaveBeenCalledOnce();
    rerender(section);
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Alerts" }));
  });
});

describe("maintenance windows", () => {
  it("mutes an alert from now for an hour, unless told otherwise, the focus back on its button", async () => {
    render(<MuteButton projectId={PROJECT} ruleId={RULE} name="Slow calls" />);
    const button = screen.getByRole("button", { name: "Mute Slow calls" });
    fireEvent.click(button);
    let dialog = screen.getByRole("dialog", { name: 'Mute "Slow calls"' });
    expect(within(dialog).getByRole("combobox", { name: "Starts" })).toHaveTextContent("Now");
    expect(within(dialog).getByRole("combobox", { name: "Lasts" })).toHaveTextContent("1 hour");
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute" }));
    await waitFor(() =>
      expect(muteAction).toHaveBeenCalledWith(PROJECT, { rule_id: RULE, starts_at: null, minutes: 60 }),
    );
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(button);

    // From a set time, read on the browser's clock and sent as the instant it names.
    fireEvent.click(button);
    dialog = screen.getByRole("dialog", { name: 'Mute "Slow calls"' });
    pick(within(dialog).getByRole("combobox", { name: "Starts" }), "later");
    const startsAt = within(dialog).getByLabelText("Starts at");
    expect(startsAt).not.toHaveAttribute("aria-label");
    expect(dialog.querySelector(`label[for="${startsAt.id}"]`)).not.toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute" }));
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Set when the window starts.");
    fireEvent.change(within(dialog).getByLabelText("Starts at"), { target: { value: "2026-10-27T09:30" } });
    pick(within(dialog).getByRole("combobox", { name: "Lasts" }), "1440");
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute" }));
    await waitFor(() => expect(muteAction).toHaveBeenCalledTimes(2));
    expect(muteAction).toHaveBeenLastCalledWith(PROJECT, {
      rule_id: RULE,
      starts_at: new Date("2026-10-27T09:30").toISOString(),
      minutes: 1440,
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());

    fireEvent.click(button);
    dialog = screen.getByRole("dialog", { name: 'Mute "Slow calls"' });
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(document.activeElement).toBe(button);
  });

  it("mutes every alert, and ends a window now", async () => {
    render(<MuteButton projectId={PROJECT} ruleId={null} name={null} />);
    fireEvent.click(screen.getByRole("button", { name: "Mute every alert" }));
    const dialog = screen.getByRole("dialog", { name: "Mute every alert" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute" }));
    await waitFor(() =>
      expect(muteAction).toHaveBeenCalledWith(PROJECT, { rule_id: null, starts_at: null, minutes: 60 }),
    );
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("button", { name: "Mute every alert" })),
    );

    const heading = <h3 id={alertHeading(RULE)} tabIndex={-1}>Slow calls</h3>;
    const { rerender } = render(
      <>
        {heading}
        <EndMute projectId={PROJECT} muteId="m-1" label="End the window muting Slow calls" landing={alertHeading(RULE)} />
      </>,
    );
    const end = screen.getByRole("button", { name: "End the window muting Slow calls" });
    end.focus();
    fireEvent.click(end);
    await waitFor(() => expect(endMuteAction).toHaveBeenCalledWith(PROJECT, "m-1"));
    await waitFor(() => expect(end).toBeEnabled());
    rerender(heading);
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Slow calls" }));
  });
});

describe("an incident's controls", { timeout: 15_000 }, () => {
  const props = {
    projectId: PROJECT,
    ruleId: RULE,
    incidentId: "i-1",
    name: "Slow calls",
    openedAt: "2026-10-26T11:00:00Z",
  };
  // Its alert's heading, which the focus falls back to.
  const heading = (
    <h3 id={`alert-${RULE}-name`} tabIndex={-1}>
      Slow calls
    </h3>
  );

  // ⚠ Each move takes away its own button: the focus goes on to the row's
  // next control, and to its alert's heading once there is none.
  it("acknowledges an open incident and resolves one once confirmed, the focus going on", async () => {
    const { rerender } = render(
      <>
        {heading}
        <IncidentActions {...props} status="open" />
      </>,
    );
    const acknowledge = screen.getByRole("button", { name: /^Acknowledge the incident of Slow calls, opened \S/ });
    acknowledge.focus();
    fireEvent.click(acknowledge);
    await waitFor(() => expect(incidentAction).toHaveBeenCalledWith(PROJECT, "i-1", "acknowledge"));
    const resolve = screen.getByRole("button", { name: /^Resolve the incident of Slow calls, opened \S/ });
    await waitFor(() => expect(resolve).toBeEnabled());
    // The refresh draws it taken.
    rerender(
      <>
        {heading}
        <IncidentActions {...props} status="acknowledged" />
      </>,
    );
    expect(document.activeElement).toBe(resolve);

    fireEvent.click(resolve);
    const confirm = await screen.findByRole("alertdialog");
    fireEvent.click(within(confirm).getByRole("button", { name: "Resolve" }));
    await waitFor(() => expect(incidentAction).toHaveBeenCalledWith(PROJECT, "i-1", "resolve"));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(resolve).toBeEnabled());
    rerender(
      <>
        {heading}
        <IncidentActions {...props} status="resolved" />
      </>,
    );
    expect(document.activeElement).toBe(screen.getByRole("heading", { name: "Slow calls" }));
  });

  it("offers no acknowledgement once taken, and nothing once resolved", () => {
    const { unmount } = render(<IncidentActions {...props} status="acknowledged" />);
    expect(screen.queryByRole("button", { name: /Acknowledge/ })).toBeNull();
    expect(screen.getByRole("button", { name: /Resolve/ })).toBeInTheDocument();
    unmount();
    render(<IncidentActions {...props} status="resolved" />);
    expect(screen.queryByRole("button")).toBeNull();
    expect(screen.queryByRole("link")).toBeNull();
  });
});

// ⚠ Another member deleted the alert: the refresh takes its card away, with
// what a control on it would say and the focus it held.
describe("a refusal for what is gone", { timeout: 15_000 }, () => {
  const GONE = { ok: false, error: "Not Found: no alert with that id in this project", gone: true } as const;
  const section = (
    <h2 id={ALERTS_HEADING} tabIndex={-1}>
      Alerts
    </h2>
  );
  const page = () => screen.getByRole("heading", { name: "Alerts" });

  it("is said in a toast, and the focus goes to the page's heading as the card goes", async () => {
    pauseAlertAction.mockResolvedValueOnce(GONE);
    const { rerender } = render(
      <>
        {section}
        <AlertActions projectId={PROJECT} alert={ALERT} connections={CONNECTIONS} />
      </>,
    );
    const pause = screen.getByRole("button", { name: "Pause Slow calls" });
    pause.focus();
    fireEvent.click(pause);
    await waitFor(() => expect(toast.error).toHaveBeenCalledWith(GONE.error));
    await waitFor(() => expect(pause).toBeEnabled());
    rerender(section);
    expect(document.activeElement).toBe(page());

    // Edits saved over it, the panel going with the card.
    saveAlertAction.mockResolvedValueOnce(GONE);
    rerender(
      <>
        {section}
        <AlertActions projectId={PROJECT} alert={ALERT} connections={CONNECTIONS} />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Edit Slow calls" }));
    const save = within(screen.getByRole("dialog")).getByRole("button", { name: "Save alert" });
    fireEvent.click(save);
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(save).toBeEnabled());
    rerender(section);
    expect(document.activeElement).toBe(page());

    incidentAction.mockResolvedValueOnce(GONE);
    rerender(
      <>
        {section}
        <IncidentActions
          projectId={PROJECT}
          ruleId={RULE}
          incidentId="i-1"
          status="open"
          name="Slow calls"
          openedAt="2026-10-26T11:00:00Z"
        />
      </>,
    );
    const acknowledge = screen.getByRole("button", { name: /^Acknowledge the incident of Slow calls/ });
    acknowledge.focus();
    fireEvent.click(acknowledge);
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(3));
    await waitFor(() => expect(acknowledge).toBeEnabled());
    rerender(section);
    expect(document.activeElement).toBe(page());

    // Its own window's End, whose landing, the alert's heading, went too.
    endMuteAction.mockResolvedValueOnce(GONE);
    rerender(
      <>
        {section}
        <EndMute projectId={PROJECT} muteId="m-1" label="End the window muting Slow calls" landing={alertHeading(RULE)} />
      </>,
    );
    const end = screen.getByRole("button", { name: "End the window muting Slow calls" });
    end.focus();
    fireEvent.click(end);
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(4));
    await waitFor(() => expect(end).toBeEnabled());
    rerender(section);
    expect(document.activeElement).toBe(page());

    muteAction.mockResolvedValueOnce(GONE);
    rerender(
      <>
        {section}
        <MuteButton projectId={PROJECT} ruleId={RULE} name="Slow calls" />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Mute Slow calls" }));
    const dialog = screen.getByRole("dialog", { name: 'Mute "Slow calls"' });
    fireEvent.click(within(dialog).getByRole("button", { name: "Mute" }));
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(5));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Mute" })).toBeEnabled());
    rerender(section);
    await waitFor(() => expect(document.activeElement).toBe(page()));
    expect(toast.error).toHaveBeenCalledTimes(5);
  });
});
