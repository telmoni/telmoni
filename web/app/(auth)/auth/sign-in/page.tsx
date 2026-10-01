import Link from "next/link";

import { buttonVariants } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getSignInConfig, type SignInConfig } from "@/lib/auth/oidc";

import { one, pendingSignIn } from "../_accounts/pending";
import { SignInForm } from "./_form";

export const metadata = { title: "Sign in" };
export const dynamic = "force-dynamic";

// What the page offers when auth cannot say: the form alone, whose action
// answers for auth being down on the first press.
const FORM_ALONE: SignInConfig = {
  passwordSignIn: true,
  allowSignUp: false,
  verifyEmail: false,
  external: null,
};

// Where `/auth/login` sends the browser while the login form is on: the
// email and password fields, and, beside them, the external provider when
// the deployment has one. The gate sends a visit that did not come through
// the door back to it.
export default async function SignInPage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const pending = await pendingSignIn(one(params.state));
  const config = (await getSignInConfig()) ?? FORM_ALONE;

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Sign in</CardTitle>
        <CardDescription>
          {config.passwordSignIn
            ? "With the email address and password on your account."
            : `Through ${config.external?.name ?? "your identity provider"}.`}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3">
        {config.passwordSignIn && (
          <SignInForm state={pending.state} email={one(params.email) ?? ""} />
        )}
        {config.external && (
          <div className="grid gap-3">
            {config.passwordSignIn && (
              <p className="text-center text-xs uppercase text-muted-foreground">or</p>
            )}
            {/* A Route Handler, not a page: it carries the pending sign-in on
                to the provider, so a client-side navigation would lose it. */}
            {/* eslint-disable-next-line @next/next/no-html-link-for-pages */}
            <a href="/auth/login/external" className={buttonVariants({ variant: "outline" })}>
              Continue with {config.external.name}
            </a>
          </div>
        )}
        {config.passwordSignIn && (
          <p className="text-sm text-muted-foreground">
            <Link href="/auth/forgot" className="underline underline-offset-4">
              Forgot your password?
            </Link>
            {config.allowSignUp && (
              <>
                {" · "}
                <Link href="/auth/signup" className="underline underline-offset-4">
                  Create an account
                </Link>
              </>
            )}
          </p>
        )}
      </CardContent>
    </Card>
  );
}
