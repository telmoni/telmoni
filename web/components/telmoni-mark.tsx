import { cn } from "@/lib/utils";

/** The compact brand mark without the favicon's surrounding padding. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 896 704"
      aria-hidden="true"
      className={cn("h-6 w-auto", className)}
    >
      <g transform="translate(-64 -128)">
        <g fill="currentColor">
          <rect x="192" y="128" width="128" height="704" />
          <rect x="64" y="320" width="256" height="128" />
          <path d="M256 320H416A224 224 0 0 1 640 544V832H512V544C512 482.56 477.44 448 416 448C354.56 448 320 482.56 320 544H256Z" />
          <path d="M512 544A224 224 0 0 1 960 544V640H832V544C832 482.56 797.44 448 736 448C674.56 448 640 482.56 640 544Z" />
        </g>
        <rect x="832" y="704" width="128" height="128" className="fill-brand-unit" />
      </g>
    </svg>
  );
}

/** The wordmark from `masters/wordmark.svg`. The word is drawn, not set: the
 *  one face in the console that is not the text's. */
export function TelmoniWordmark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 -1000 4614.38 1010.5"
      aria-hidden="true"
      className={cn("h-4 w-auto overflow-visible", className)}
    >
      <path fill="currentColor" d={WORD} />
    </svg>
  );
}

/** The mark beside the wordmark in the brand pack's lockup geometry, so the
 *  two never need aligning. The word is drawn, not set: the one face in the
 *  console that is not the text's. The box is the mark's height, and the
 *  word's round letters overshoot the baseline below it, unclipped. */
export function TelmoniLockup({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 -820 4546.17 827.35"
      aria-hidden="true"
      className={cn("h-6 w-auto overflow-visible", className)}
    >
      <g id="mark" transform="translate(0 -820) scale(0.82)">
        <MarkShapes />
      </g>
      <g id="word" transform="translate(1316.1 0) scale(0.7)">
        <path fill="currentColor" d={WORD} />
      </g>
    </svg>
  );
}

function MarkShapes() {
  return (
    <>
      <rect x="200" y="0" width="200" height="1000" fill="currentColor" />
      <rect x="0" y="300" width="400" height="180" fill="currentColor" />
      <path
        d="M300 300H525A325 325 0 0 1 850 625V1000H650V625C650 523.5 612.5 480 525 480C437.5 480 400 523.5 400 625H300Z"
        fill="currentColor"
      />
      <path
        d="M650 625A325 337.5 0 0 1 1300 625V720H1100V625C1100 523.5 1062.5 480 975 480C887.5 480 850 523.5 850 625Z"
        fill="currentColor"
      />
      <rect x="1095" y="790" width="210" height="210" className="fill-brand-unit" />
    </>
  );
}

// The pack's `masters/wordmark.svg`, its baseline at y = 0.
const WORD =
  "M171.5 -1000H343V-700H514.5V-545.65H343V0H171.5V-545.65H0V-700H171.5ZM1250.01 -455.18A347.88 360.5 0 1 0 1198.56 -137.9H917.26A166.09 202.72 0 0 1 756.13 -300.83H1261.89A347.88 360.5 0 0 0 1250.01 -455.18ZM775.27 -455.18A166.09 202.72 0 0 1 1059.25 -455.18ZM1378.34 -1000H1549.84V0H1378.34ZM1700.75 -700H1996.59A295.84 295.84 0 0 1 2203.79 -615.32A295.84 304.24 0 0 1 2712.6 -404.16V0H2541.1V-404.16C2541.1 -503.2 2503.8 -545.65 2416.77 -545.65C2329.73 -545.65 2292.43 -503.2 2292.43 -404.16V0H2120.93V-404.16C2120.93 -503.2 2083.63 -545.65 1996.59 -545.65C1909.56 -545.65 1872.25 -503.2 1872.25 -404.16V0H1700.75ZM3181.15 -710.5A360.5 360.5 0 1 1 3181.15 10.5A360.5 360.5 0 1 1 3181.15 -710.5ZM3181.15 -552.72A178.71 202.72 0 1 0 3181.15 -147.28A178.71 202.72 0 1 0 3181.15 -552.72ZM3651.41 -700H3968.68A317.27 317.27 0 0 1 4285.96 -382.73V0H4114.46V-382.73C4114.46 -496.77 4070.73 -545.65 3968.68 -545.65C3866.64 -545.65 3822.91 -496.77 3822.91 -382.73V0H3651.41ZM4438.59 -700H4610.09V0H4438.59ZM4434.31 -1000H4614.38V-819.92H4434.31Z";
