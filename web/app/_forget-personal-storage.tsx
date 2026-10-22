"use client";

import { useEffect } from "react";

import { forgetRecent } from "@/lib/search-recent";
import { forgetTrail } from "@/lib/use-console-trail";

/** Every sign-out lands on the home page signed out (`auth/logout`), so where
 *  the last person went goes with them; preferences stay, being nobody's. */
export function ForgetPersonalStorage() {
  useEffect(() => {
    forgetRecent();
    forgetTrail();
  }, []);
  return null;
}
