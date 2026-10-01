import Link from "next/link";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

import { ForgotForm } from "./_form";

export const metadata = { title: "Reset your password" };
export const dynamic = "force-dynamic";

export default function ForgotPage() {
  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Reset your password</CardTitle>
        <CardDescription>
          Enter the address on your account and we email you a link to choose a new one.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-3">
        <ForgotForm />
        <p className="text-sm text-muted-foreground">
          <Link href="/auth/login" className="underline underline-offset-4">
            Back to sign in
          </Link>
        </p>
      </CardContent>
    </Card>
  );
}
