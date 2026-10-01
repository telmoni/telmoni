import { cache } from "react";
import { getSession, type SessionData } from "@/lib/auth/session";

export const getServerSession = cache(async (): Promise<SessionData | null> => {
  return getSession();
});
