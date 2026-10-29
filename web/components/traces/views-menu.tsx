"use client";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  updateTraceViewAction,
} from "@/app/(app)/[organization]/[project]/traces/actions";
import { SavedMenu } from "@/components/traces/saved-menu";
import type {
  ListQuery,
  ListViewQuery,
  SessionView,
  TraceQuery,
  TraceView,
  UserView,
} from "@/lib/server/entities/traces";
import { SESSIONS } from "@/lib/sessions";
import { TRACES, type ListSpec, type TraceUi } from "@/lib/traces";
import { type SessionSort, type UserSort, ViewKind } from "@/lib/types/enums";
import { setTracePrefs, useTracePrefs } from "@/lib/use-trace-prefs";
import { USERS } from "@/lib/users";

type Common = {
  organization: string;
  slug: string;
  projectId: string;
  ui: TraceUi;
  /** Whether the person may save, save over and delete them: an owner or an
   *  admin, as the server holds. */
  canSave: boolean;
};

/** A list, its query, and the project's views of it, or `null` where they
 *  could not be read. */
type Props = Common &
  (
    | { list: "traces"; query: TraceQuery; views: TraceView[] | null }
    | { list: "sessions"; query: ListQuery<SessionSort>; views: SessionView[] | null }
    | { list: "users"; query: ListQuery<UserSort>; views: UserView[] | null }
  );

// The project's saved views of a list, in the saved menu (`saved-menu.tsx`):
// each opens the list on its filters, sort and window with its columns; the
// one the page was opened from is named on the button, starred once the list
// has moved off it.
export function ViewsMenu(props: Props) {
  const prefs = useTracePrefs();
  switch (props.list) {
    case "traces":
      return (
        <Menu
          {...props}
          spec={TRACES}
          kind={ViewKind.Traces}
          noun="traces"
          shown={prefs.columns}
          show={(columns) => setTracePrefs({ columns })}
        />
      );
    case "sessions":
      return (
        <Menu
          {...props}
          spec={SESSIONS}
          kind={ViewKind.Sessions}
          noun="sessions"
          shown={prefs.sessionColumns}
          show={(sessionColumns) => setTracePrefs({ sessionColumns })}
        />
      );
    case "users":
      return (
        <Menu
          {...props}
          spec={USERS}
          kind={ViewKind.Users}
          noun="users"
          shown={prefs.userColumns}
          show={(userColumns) => setTracePrefs({ userColumns })}
        />
      );
  }
}

type View<K, S extends string> = {
  id: string;
  name: string;
  query: ListViewQuery<S>;
  columns: K[];
};

function Menu<K extends string, S extends string, V extends View<K, S>>({
  organization,
  slug,
  projectId,
  ui,
  canSave,
  query,
  views,
  spec,
  kind,
  noun,
  shown,
  show,
}: Common & {
  query: ListQuery<S>;
  views: V[] | null;
  spec: ListSpec<K, S>;
  kind: typeof ViewKind.Traces | typeof ViewKind.Sessions | typeof ViewKind.Users;
  /** What the rows are, in the delete dialog's words. */
  noun: string;
  /** The columns the browser shows, and how it is told to show others. */
  shown: readonly string[];
  show: (columns: K[]) => void;
}) {
  const current = views?.find((v) => v.id === ui.view);
  const columns = spec.ordered(shown);
  const columnsMoved = current !== undefined && current.columns.join() !== columns.join();
  const edited = current !== undefined && (!spec.showsView(query, current) || columnsMoved);
  // The list as it stands, under `name`.
  const input = (name: string) => ({ name, kind, query: spec.viewQueryOf(query), columns });

  return (
    <SavedMenu
      noun="view"
      saved={views}
      current={current}
      edited={edited}
      canSave={canSave}
      hrefOf={(view) => spec.viewHref(organization, slug, view, { display: ui.display })}
      // Its columns are the browser's, set on any click, a new tab's included.
      onFollow={(view) => show(spec.ordered(view.columns))}
      saveNew={(name) => saveTraceViewAction(projectId, input(name))}
      saveOver={(view) => updateTraceViewAction(projectId, view.id, input(view.name))}
      rename={(view, name) =>
        updateTraceViewAction(projectId, view.id, {
          name,
          kind,
          query: view.query,
          columns: view.columns,
        })
      }
      remove={(view) => deleteTraceViewAction(projectId, view.id)}
      words={{
        holds: "this list's filters, sort, window and columns",
        keeps: "its filters, sort and columns",
        whoSaves: "Owners and admins save views; everyone on the project applies them.",
        deleting: () => `It goes for everyone on the project. The ${noun} it showed are not touched.`,
      }}
    />
  );
}
