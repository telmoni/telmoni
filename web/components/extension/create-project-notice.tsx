import type { ReactNode } from "react";

/**
 * What New project says at the plan's project ceiling, where a plan the
 * deployment sells holds more (`components/create-project.tsx`): nothing
 * here, and the file a console built on this one lays its own copy over to
 * say what lifts it there (see `lib/extension/ui.ts`), with fields of its own
 * that `lib/extension/create-project.ts` reads by name. Its copy may be an
 * async server component that reads what it needs; the layout passes nothing.
 */
export function ExtensionCreateProjectNotice(): ReactNode {
  return null;
}
