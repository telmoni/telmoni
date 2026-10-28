import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it, vi } from "vitest";
import { z } from "zod";

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
import { AlertListSchema, IncidentListSchema, MuteListSchema } from "./alerts";
import { ContentModeSettingSchema } from "./content-mode";
import { FieldInventorySchema } from "./field-inventory";
import { OrganizationHealthSchema } from "./organization-health";
import { SessionLayoutSettingSchema } from "./session-layout";
import { SessionDetailSchema, SessionListSchema } from "./sessions";
import {
  FacetsSchema,
  SavedViewOfAnyKindSchema,
  TraceDetailSchema,
  TraceHistogramSchema,
  TraceLayoutListSchema,
  TraceListSchema,
  TraceViewListSchema,
} from "./traces";
import { UserListSchema } from "./users";

// Every answer the server's suites pin under `contract/responses`, written
// from its struct (`make contract`), by the schema the console reads it with.
const READERS: Record<string, z.ZodType> = {
  "telemetry/alerts.json": AlertListSchema,
  "telemetry/content-mode.json": ContentModeSettingSchema,
  "telemetry/facets.json": FacetsSchema,
  "telemetry/field-inventory.json": FieldInventorySchema,
  "telemetry/health.json": OrganizationHealthSchema,
  "telemetry/histogram.json": TraceHistogramSchema,
  "telemetry/incidents.json": IncidentListSchema,
  "telemetry/layouts.json": TraceLayoutListSchema,
  "telemetry/mutes.json": MuteListSchema,
  "telemetry/session-layout.json": SessionLayoutSettingSchema,
  "telemetry/session.json": SessionDetailSchema,
  "telemetry/sessions.json": SessionListSchema,
  "telemetry/trace.json": TraceDetailSchema,
  "telemetry/traces.json": TraceListSchema,
  "telemetry/users.json": UserListSchema,
  "telemetry/views.json": TraceViewListSchema,
};

const PINNED = fileURLToPath(new URL("../../../../contract/responses/", import.meta.url));

function pinned(name: string): unknown {
  return JSON.parse(readFileSync(`${PINNED}${name}`, "utf8"));
}

describe("the server's answers", () => {
  it.each(Object.entries(READERS))("%s reads", (name, schema) => {
    expect(schema.safeParse(pinned(name)).error).toBeUndefined();
  });

  it("each has a reader", () => {
    const files = readdirSync(PINNED, { recursive: true, encoding: "utf8" })
      .filter((file) => file.endsWith(".json"))
      .sort();
    expect(files).toEqual(Object.keys(READERS).sort());
  });

  it("a saved view of either kind reads as a save answers it", () => {
    const Views = z.object({ views: z.array(z.unknown()) });
    for (const name of ["telemetry/views.json", "telemetry/layouts.json"]) {
      for (const view of Views.parse(pinned(name)).views) {
        expect(SavedViewOfAnyKindSchema.safeParse(view).error).toBeUndefined();
      }
    }
  });
});
