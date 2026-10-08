import type { ReactNode } from "react";

/**
 * A badge beside the active organization's name in the header's switcher
 * (`components/resource-selector.tsx`): nothing here, and the file a console
 * built on this one lays its own copy over to show the organization's plan
 * there (see `lib/extension/ui.ts`). Its copy may be an async server
 * component that reads what it needs itself; the layout passes nothing.
 */
export function ExtensionOrganizationBadge(): ReactNode {
  return null;
}
