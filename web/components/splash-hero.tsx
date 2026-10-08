import { Mark } from "@/components/mark";
import { cn } from "@/lib/utils";

// The hero's blocks arrive in reading order, a beat apart: a short rise and
// a fade, each held hidden until its turn, and none under reduced motion.
const ENTER =
  "animate-in fade-in slide-in-from-bottom-2 fill-mode-both duration-700 ease-out motion-reduce:animate-none";

// The claim with its one phrase marked, the stroke drawn once the claim has
// settled.
function Claim({ text, highlight }: { text: string; highlight: string }) {
  const at = text.indexOf(highlight);
  if (at < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, at)}
      <Mark sweep="load">{highlight}</Mark>
      {text.slice(at + highlight.length)}
    </>
  );
}

// The hero as a product site frames one: the claim, on a dotted field, with
// the way in under it. The product shot — a screenshot of the console — is
// still to be taken, and takes the column beside the claim when it is. The
// copy and the way in come from the page.
export function SplashHero({
  claim,
  highlight,
  pitch,
  status,
  captions,
  children,
}: {
  claim: string;
  highlight: string;
  pitch: string;
  status: string;
  captions: readonly string[];
  children: React.ReactNode;
}) {
  return (
    <div className="dots w-full rounded-xl border border-border p-5 sm:p-8">
      <div className="flex max-w-3xl min-w-0 flex-col">
        <p
          className={cn(
            ENTER,
            "flex flex-wrap gap-x-5 gap-y-1 font-mono text-[11px] uppercase tracking-label text-muted-foreground",
          )}
        >
          {captions.map((caption) => (
            <span key={caption}>{caption}</span>
          ))}
        </p>
        <h1
          className={cn(
            ENTER,
            "delay-100 mt-5 text-[clamp(2rem,3.6vw,3rem)] font-semibold leading-[1.05] tracking-[-0.035em] text-foreground",
          )}
        >
          <Claim text={claim} highlight={highlight} />
        </h1>
        <p className={cn(ENTER, "delay-200 mt-5 max-w-[52ch] text-[15px] leading-[1.5] text-muted-foreground")}>
          {pitch}
        </p>
        <div className={cn(ENTER, "delay-300 mt-6")}>{children}</div>
        <p
          className={cn(
            ENTER,
            "delay-400 mt-6 max-w-[60ch] border-t border-border pt-4 text-sm leading-relaxed text-muted-foreground",
          )}
        >
          {status}
        </p>
      </div>
    </div>
  );
}
