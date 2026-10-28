import type { z } from "zod";

import { type RouteRead, UNREACHED, routeReadOf } from "@/lib/route-read";

/** A Route Handler read in the browser, parsed as `schema`: made here rather
 *  than in `lib/`, where a bare fetch is a server's. The proxy's redirect to
 *  sign in is never followed, since followed it is the sign-in page. */
export async function readRoute<T>(
  url: string,
  schema: z.ZodType<T>,
  init: Omit<RequestInit, "headers"> & { headers?: Record<string, string> } = {},
): Promise<RouteRead<T>> {
  try {
    const res = await fetch(url, {
      cache: "no-store",
      redirect: "manual",
      ...init,
      headers: { accept: "application/json", ...init.headers },
    });
    return await routeReadOf(res, schema);
  } catch {
    return UNREACHED;
  }
}
