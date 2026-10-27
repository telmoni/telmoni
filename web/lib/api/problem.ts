import { z } from "zod";

export const Problem = z
  .object({
    type: z.string().optional(),
    title: z.string(),
    status: z.number(),
    detail: z.string().optional(),
    instance: z.string().optional(),
  })
  .passthrough();

export type Problem = z.infer<typeof Problem>;

/** The problem types the console words itself, each one a service answers
 *  (`crates/shared/tests/error_catalog.rs` reads this table). */
export const ProblemType = {
  BadUrl:                "/errors/auth/bad-url",
  NotFound:              "/errors/auth/not-found",
  SignUpClosed:          "/errors/auth/sign-up-closed",
  SsoRequired:           "/errors/auth/sso-required",
  FeatureOff:            "/errors/tenant/feature-off",
  AgentDisabled:         "/errors/agent/disabled",
  AgentRateLimited:      "/errors/agent/rate-limited",
  AgentModelUnavailable: "/errors/agent/model-unavailable",
} as const;

