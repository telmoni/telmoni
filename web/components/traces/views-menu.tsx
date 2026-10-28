"use client";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  updateTraceViewAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { SavedMenu } from "@/components/traces/saved-menu";
import type { TraceQuery, TraceView } from "@/lib/server/entities/traces";
import { orderedColumns, showsView, viewHref, viewQueryOf, type TraceUi } from "@/lib/traces";
import { ViewKind } from "@/lib/types/enums";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";

type Props = {
  organization: string;
  slug: string;
  projectId: string;
  query: TraceQuery;
  ui: TraceUi;
  /** The project's views, or `null` where they could not be read. */
  views: TraceView[] | null;
  /** Whether the person may save, save over and delete them: an owner or an
   *  admin, as the server holds. */
  canSave: boolean;
};

// The project's saved views, in the saved menu (`saved-menu.tsx`): each
// opens the list on its filters, sort and window with its columns; the one
// the page was opened from is named on the button, starred once the list has
// moved off it.
export function ViewsMenu({ organization, slug, projectId, query, ui, views, canSave }: Props) {
  const prefs = useTracePrefs();
  const current = views?.find((v) => v.id === ui.view);
  const columnsMoved =
    current !== undefined && current.columns.join() !== orderedColumns(prefs.columns).join();
  const edited = current !== undefined && (!showsView(query, current) || columnsMoved);
  // The list as it stands, under `name`.
  const input = (name: string) => ({
    name,
    kind: ViewKind.List,
    query: viewQueryOf(query),
    columns: orderedColumns(prefs.columns),
  });

  return (
    <SavedMenu
      noun="view"
      saved={views}
      current={current}
      edited={edited}
      canSave={canSave}
      hrefOf={(view) => viewHref(organization, slug, view, { display: ui.display })}
      // Its columns are the browser's, set on any click, a new tab's included.
      onFollow={(view) => setTracePrefs({ columns: orderedColumns(view.columns) })}
      saveNew={(name) => saveTraceViewAction(projectId, input(name))}
      saveOver={(view) => updateTraceViewAction(projectId, view.id, input(view.name))}
      rename={(view, name) =>
        updateTraceViewAction(projectId, view.id, {
          name,
          kind: ViewKind.List,
          query: view.query,
          columns: view.columns,
        })
      }
      remove={(view) => deleteTraceViewAction(projectId, view.id)}
      words={{
        holds: "this list's filters, sort, window and columns",
        keeps: "its filters, sort and columns",
        whoSaves: "Owners and admins save views; everyone on the project applies them.",
        deleting: () =>
          "It goes for everyone on the project. The traces it showed are not touched.",
      }}
    />
  );
}
