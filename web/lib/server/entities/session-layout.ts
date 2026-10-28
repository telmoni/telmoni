import { cache } from "react";
import { z } from "zod";

import { valueOf } from "./read";
import { readLane } from "./traces";

// The layout a project's sessions and traces open in: the saved layout
// `view_id` names while it stands, or the console's own default
// (`lib/layout.ts`).
export const SessionLayoutSettingSchema = z.object({
  view_id: z.string().nullable(),
});
export type SessionLayoutSetting = z.infer<typeof SessionLayoutSettingSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its layout, so no refusal is expected here.
export const fetchSessionLayout = cache(
  async (projectId: string): Promise<SessionLayoutSetting | null> => {
    return valueOf(
      await readLane(
        "fetchSessionLayout",
        projectId,
        "/session-layout",
        SessionLayoutSettingSchema,
      ),
    );
  },
);
