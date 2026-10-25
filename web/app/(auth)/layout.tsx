import Link from "next/link";

import { TelmoniLockup } from "@/components/telmoni-mark";
import { PRODUCT_NAME } from "@/lib/site";

export default function AuthLayout({ children }: { children: React.ReactNode }) {
  return (
    <div className="theme-paper min-h-screen flex flex-col items-center justify-center bg-background p-8 gap-12">
      <Link href="/" aria-label={PRODUCT_NAME} className="flex items-center">
        <TelmoniLockup className="h-5" />
      </Link>
      <main className="w-full max-w-md">{children}</main>
    </div>
  );
}
