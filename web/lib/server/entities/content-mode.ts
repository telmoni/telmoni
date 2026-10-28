import { cache } from "react";
import { z } from "zod";

import { ContentMode } from "@/lib/types/enums";

import { valueOf } from "./read";
import { readLane } from "./traces";

// A project's content mode, and the modes its switch offers. The server
// decides both, so a mode it starts offering needs no change here.
export const ContentModeSettingSchema = z.object({
  content_mode: z.enum(ContentMode),
  offered: z.array(z.enum(ContentMode)),
});
export type ContentModeSetting = z.infer<typeof ContentModeSettingSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its mode, so no refusal is expected here.
export const fetchContentMode = cache(
  async (projectId: string): Promise<ContentModeSetting | null> => {
    return valueOf(
      await readLane("fetchContentMode", projectId, "/content-mode", ContentModeSettingSchema),
    );
  },
);
