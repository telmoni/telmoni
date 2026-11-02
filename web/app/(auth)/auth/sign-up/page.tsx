import { notFound } from "next/navigation";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getSignInConfig } from "@/lib/auth/oidc";

import { pendingSignIn } from "../_accounts/pending";
import { sole } from "@/lib/list-query";
import { SignUpForm } from "./_form";

export const metadata = { title: "Create an account" };

// Where `/auth/signup` sends the browser while the login form is on. Reached
// with sign-ups closed too: an invitation's "Create an account" lands here,
// and auth admits the address the invitation was sent to.
export default async function SignUpPage({
  searchParams,
}: PageProps<"/auth/sign-up">) {
  // With the password form off its lanes are not mounted, so there is no
  // page, whatever state a sign-in in progress sealed; when auth cannot say,
  // the form's action answers.
  const config = await getSignInConfig();
  if (config && !config.passwordSignIn) notFound();
  const params = await searchParams;
  const pending = await pendingSignIn(sole(params.state));
  const verifyEmail = config?.verifyEmail ?? false;
  const byInvitation = config ? !config.allowSignUp : false;

  return (
    <Card className="gap-gutter">
      <CardHeader>
        <CardTitle>Create an account</CardTitle>
        <CardDescription>
          {byInvitation
            ? "Sign-ups are by invitation: use the address your invitation was sent to."
            : verifyEmail
              ? "We send a link to confirm your address before the account can be used."
              : "Your account is ready as soon as you press the button."}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-gutter">
        <SignUpForm
          state={pending.state}
          email={sole(params.email) ?? ""}
          verifyEmail={verifyEmail}
        />
        <p className="text-sm text-muted-foreground">
          Already have one?{" "}
          {/* A Route Handler, not a page, so never a <Link>: a link's prefetch
              would run it, starting another sign-in over this sign-up. */}
          {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
          <a href="/auth/login" className="underline underline-offset-4">
            Sign in
          </a>
        </p>
      </CardContent>
    </Card>
  );
}
