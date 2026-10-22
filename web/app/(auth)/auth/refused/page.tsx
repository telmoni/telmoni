import { buttonVariants } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { refusalNamed, type Refusal } from "@/lib/auth/refusal";

import { one } from "../_accounts/pending";

export const metadata = { title: "Not signed in" };

const WORDS: Record<Refusal, string> = {
  closed:
    "New people join this deployment by invitation. Ask an owner or admin of your organization to invite you, then sign in again.",
  taken:
    "An account here already holds that email address. Sign in the way you did before.",
  unverified:
    "Your identity provider has not verified your email address, which this deployment needs before you sign in.",
  provider: "Your identity provider did not sign you in.",
  expired: "This sign-in expired, or was started in another tab.",
  failed: "Sign-in did not finish. Try again in a moment.",
};

// Where the callback sends a sign-in that ended with no session: why, in a
// sentence, and the way back in, which waits for the person to press it.
export default async function RefusedPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const reason = refusalNamed(one((await searchParams).reason));
  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>You are not signed in</CardTitle>
        <CardDescription>{WORDS[reason]}</CardDescription>
      </CardHeader>
      <CardContent>
        {/* A Route Handler, not a page: a <Link>'s prefetch would start a sign-in. */}
        {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
        <a href="/auth/login" className={buttonVariants({ variant: "outline" })}>
          Try again
        </a>
      </CardContent>
    </Card>
  );
}
