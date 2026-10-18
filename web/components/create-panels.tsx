"use client";

import type { ReactNode } from "react";
import { createPortal } from "react-dom";

import { useConsoleShell } from "@/components/console-shell";
import { useConsoleUi } from "@/components/console-ui-context";
import { CreateOrganizationPanel } from "@/components/create-organization";
import { CreateProjectPanel, useCanCreateProjects } from "@/components/create-project";

/** New project and New organization, opened from the switcher's menus and
 *  the Projects page (`openCreate`) and drawn in the content area as a
 *  peeked trace is. */
export function CreatePanels({
  host,
  notice,
}: {
  /// The console's host, ahead of a new organization's URL.
  host: string;
  /// What a console built on this one says in New organization
  /// (`components/extension/create-organization-notice.tsx`).
  notice?: ReactNode;
}) {
  const { element } = useConsoleShell();
  const { creating, closeCreate } = useConsoleUi();
  const canCreateProjects = useCanCreateProjects();
  if (!element || !creating) return null;

  // Keyed by the opening, so New project asked for again over an open one
  // comes fresh and focused, as a dialog opened again did.
  const returnFocus = () => creating.returnTo;
  if (creating.kind === "project") {
    return canCreateProjects
      ? createPortal(
          <CreateProjectPanel key={creating.opened} onClose={closeCreate} returnFocus={returnFocus} />,
          element,
        )
      : null;
  }
  return createPortal(
    <CreateOrganizationPanel
      key={creating.opened}
      host={host}
      notice={notice}
      onClose={closeCreate}
      returnFocus={returnFocus}
    />,
    element,
  );
}
