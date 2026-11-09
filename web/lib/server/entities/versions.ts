import { VersionsSchema, versionsQuery, type Versions, type VersionsAsk } from "@/lib/versions";

import type { Read } from "./read";
import { readLane } from "./traces";

export { VersionsSchema };

/** A query the server refused: an agent or a prompt out of its bounds. */
const REFUSED = { 400: "refused" } as const;

/** One agent's releases, its models or a prompt's versions side by side, as
 *  the Agents page's panel reads them. */
export function fetchVersions(projectId: string, ask: VersionsAsk): Promise<Read<Versions, "refused">> {
  return readLane("fetchVersions", projectId, `/agent-versions?${versionsQuery(ask).toString()}`, VersionsSchema, REFUSED);
}
