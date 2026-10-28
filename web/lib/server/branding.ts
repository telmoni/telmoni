// Who runs THIS deployment, read at request time so one image serves every
// operator. The product's own name and docs stay constants in `lib/site.ts`;
// nothing here is the product's. Every value is optional, and a surface that
// needs one and has none draws nothing rather than a placeholder.
import "server-only";

import { env } from "@/lib/env";
import { PRODUCT_NAME } from "@/lib/site";

export const branding = {
  // The legal entity behind the deployment, for the copyright line and the
  // organization record in structured data. Falls back to the product name.
  get COMPANY_NAME(): string {
    return env.COMPANY_NAME ?? PRODUCT_NAME;
  },
  // Where a person writes to. Unset, no page offers an address.
  get SUPPORT_EMAIL(): string | undefined {
    return env.SUPPORT_EMAIL;
  },
  // The base under which the deployment publishes `privacy-policy` and
  // `terms-of-service`. Unset, the footer offers no legal links and `/legal/*`
  // answers 404 (`proxy.ts`).
  get LEGAL_URL(): string | undefined {
    return env.LEGAL_URL;
  },
} as const;
