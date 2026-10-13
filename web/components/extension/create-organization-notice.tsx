import type { ReactNode } from "react";

/**
 * What the create-organization dialog says under its description
 * (`components/create-organization.tsx`): nothing here, and the file a
 * console built on this one lays its own copy over to say what a new
 * organization starts on there (see `lib/extension/ui.ts`). Its copy may be
 * an async server component that reads what it needs itself; the layout
 * passes nothing.
 */
export function ExtensionCreateOrganizationNotice(): ReactNode {
  return null;
}
