import type { ReactNode } from "react";

/**
 * A badge beside an organization's name in the header's switcher
 * (`components/resource-selector.tsx`), on the trigger and on its row:
 * nothing here, and the file a console built on this one lays its own copy
 * over to show the organization's plan there (see `lib/extension/ui.ts`).
 * Its copy may be an async server component that reads what it needs by the
 * id; the layout draws one per organization the person is in.
 */
export function ExtensionOrganizationBadge({ organizationId }: { organizationId: string }): ReactNode {
  void organizationId;
  return null;
}
