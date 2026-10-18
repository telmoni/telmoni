import { z } from "zod";

/** The newest notice a page drew, by its `created_at`, as a Server Action
 *  takes it: an ISO time with its offset, passed on as it came, since the
 *  service compares it with the microseconds the row carries. */
export const NewestDrawn = z.iso.datetime({ offset: true });

export const NOT_DRAWN = "Couldn't tell what the page showed — reload and try again.";
