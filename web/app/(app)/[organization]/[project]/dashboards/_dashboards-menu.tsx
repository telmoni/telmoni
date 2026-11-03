"use client";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  updateTraceViewAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { SavedMenu } from "@/components/traces/saved-menu";
import { dashboardsHref, sameCharts, type Chart } from "@/lib/dashboards";
import { ViewKind } from "@/lib/types/enums";

/** A saved dashboard as the page reads it: its charts checked as the URL's are. */
export type SavedDashboard = { id: string; name: string; charts: Chart[] };

// The project's saved dashboards, in the saved menu (`saved-menu.tsx`): each
// opens the page on its charts over the window shown; the one the page was
// opened from is named on the button, starred once its charts have moved.
export function DashboardsMenu({
  organization,
  slug,
  projectId,
  days,
  charts,
  dashboards,
  current,
  canSave,
}: {
  organization: string;
  slug: string;
  projectId: string;
  days: number;
  /** The charts the page draws now. */
  charts: readonly Chart[];
  /** The project's, or `null` where they could not be read. */
  dashboards: SavedDashboard[] | null;
  current: SavedDashboard | undefined;
  /** An owner or an admin, as the server holds. */
  canSave: boolean;
}) {
  const input = (name: string, of: readonly Chart[]) => ({
    name,
    kind: ViewKind.Dashboard,
    query: { charts: of },
    columns: [],
  });
  return (
    <SavedMenu
      noun="dashboard"
      saved={dashboards}
      current={current}
      edited={current !== undefined && !sameCharts(charts, current.charts)}
      canSave={canSave}
      hrefOf={(dashboard) => dashboardsHref(organization, slug, days, dashboard.charts, dashboard.id)}
      saveNew={(name) => saveTraceViewAction(projectId, input(name, charts))}
      saveOver={(dashboard) => updateTraceViewAction(projectId, dashboard.id, input(dashboard.name, charts))}
      rename={(dashboard, name) => updateTraceViewAction(projectId, dashboard.id, input(name, dashboard.charts))}
      remove={(dashboard) => deleteTraceViewAction(projectId, dashboard.id)}
      words={{
        holds: "the charts this page draws",
        keeps: "its charts",
        whoSaves: "Owners and admins save dashboards; everyone on the project opens them.",
        deleting: () => "It goes for everyone on the project. No span or total is touched.",
      }}
    />
  );
}
