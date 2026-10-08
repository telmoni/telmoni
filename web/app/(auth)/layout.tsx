import Link from "next/link";

import { TelmoniMark } from "@/components/telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";

export default function AuthLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="theme-paper min-h-screen flex flex-col items-center justify-center bg-background p-8 gap-12">
      <Link href="/" className="flex items-center gap-2 text-sm font-semibold tracking-tight">
        <TelmoniMark className="size-4" />
        {PRODUCT_NAME}
      </Link>
      <main className="w-full max-w-md">{children}</main>
    </div>
  );
}
