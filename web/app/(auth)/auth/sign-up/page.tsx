import Link from "next/link";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getSignInConfig } from "@/lib/auth/oidc";

import { one, pendingSignIn } from "../_accounts/pending";
import { SignUpForm } from "./_form";

export const metadata = { title: "Create an account" };
export const dynamic = "force-dynamic";

// Where `/auth/signup` sends the browser while the login form is on. Reached
// with sign-ups closed too: an invitation's "Create an account" lands here,
// and auth admits the address the invitation was sent to.
export default async function SignUpPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const pending = await pendingSignIn(one(params.state));
  const config = await getSignInConfig();
  const verifyEmail = config?.verifyEmail ?? false;
  const byInvitation = config ? !config.allowSignUp : false;

  return (
    <Card className="gap-3">
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
      <CardContent className="grid gap-3">
        <SignUpForm
          state={pending.state}
          email={one(params.email) ?? ""}
          verifyEmail={verifyEmail}
        />
        <p className="text-sm text-muted-foreground">
          Already have one?{" "}
          <Link href="/auth/login" className="underline underline-offset-4">
            Sign in
          </Link>
        </p>
      </CardContent>
    </Card>
  );
}
