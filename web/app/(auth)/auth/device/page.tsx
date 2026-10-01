import { redirect } from "next/navigation";

import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { getServerSession } from "@/lib/server/session";

import { one } from "../_accounts/pending";
import { DeviceForm } from "./_form";

export const metadata = { title: "Sign in a device" };
export const dynamic = "force-dynamic";

// The page a device (the CLI) sends the person to, with the code it shows.
// Under `/auth`, which the proxy leaves open, so the session is checked
// here: approving is done AS somebody, and a visitor with no session signs
// in first and comes back with the code intact.
export default async function DevicePage({
  searchParams,
}: {
  searchParams: Promise<Record<string, string | string[] | undefined>>;
}) {
  const params = await searchParams;
  const code = one(params.code) ?? "";

  const session = await getServerSession();
  if (!session) {
    const here = code ? `/auth/device?code=${encodeURIComponent(code)}` : "/auth/device";
    redirect(`/auth/login?returnTo=${encodeURIComponent(here)}`);
  }

  return (
    <Card className="gap-3">
      <CardHeader>
        <CardTitle>Sign in a device</CardTitle>
        <CardDescription>
          A device is asking to sign in as <span className="font-mono">{session.email}</span>.
          Approve it only if the code below is the one it shows.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <DeviceForm code={code} />
      </CardContent>
    </Card>
  );
}
