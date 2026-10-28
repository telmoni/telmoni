import { z } from "zod";

/** What the operator broadcasts across the banner: a sentence, with a link
 *  beside it where one is given, and an id a person dismisses it by. */
const Structured = z.object({
  id: z.string().optional(),
  message: z.string(),
  linkText: z.string().optional(),
  href: z.string().optional(),
});
export type ProjectNotification = z.infer<typeof Structured>;

/** The broadcast as written: the sentence alone, or the structured form. */
export const AnnouncementSchema = z.union([
  z.string().transform((message): ProjectNotification => ({ message })),
  Structured,
]);
