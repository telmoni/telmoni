import { z } from "zod";

// A seat given to the person from an organization's roster that they have
// not seen yet, as `/me` lists it: the bell's to show until they open the
// project from there or dismiss it.
export const GivenSeatSchema = z.object({
  projectId:        z.string(),
  projectName:      z.string(),
  projectSlug:      z.string(),
  organizationId:   z.string(),
  organizationName: z.string(),
  organizationSlug: z.string(),
  role:             z.string(),
  // `null` once the person who gave it has deleted their account.
  giverEmail:       z.string().nullable(),
  giverDisplayName: z.string().nullable().optional(),
  createdAt:        z.string(),
});
export type GivenSeat = z.infer<typeof GivenSeatSchema>;
