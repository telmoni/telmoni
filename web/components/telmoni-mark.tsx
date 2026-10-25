import { cn } from "@/lib/utils";

/** The tm monogram from `app/icon.svg`, painted from tokens (`--mark-*`,
 *  `globals.css`) so it takes the shades of the surface it sits on. */
export function TelmoniMark({ className }: { className?: string }) {
  return (
    <svg viewBox="40 125 955 740" aria-hidden="true" className={cn("size-5", className)}>
      <MarkShapes />
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
      viewBox="0 -899.4 4978.8 899.4"
      aria-hidden="true"
      className={cn("h-6 w-auto overflow-visible", className)}
    >
      <g transform="translate(-48.62 -1051.32) scale(1.2154)">
        <MarkShapes />
      </g>
      <path fill="currentColor" transform="translate(1510.94 0)" d={WORD} />
    </svg>
  );
}

function MarkShapes() {
  return (
    <>
      <rect className="fill-mark-ink" x="145" y="125" width="130" height="740" rx="65" />
      <rect className="fill-mark-ink" x="40" y="335" width="350" height="120" rx="60" />
      <rect className="fill-mark-mid" x="385" y="455" width="130" height="410" rx="65" />
      <path className="fill-mark-mid" d="M385 520A185 185 0 0 1 755 520H625A55 55 0 0 0 515 520Z" />
      <rect className="fill-mark-mid" x="625" y="455" width="130" height="410" rx="65" />
      <path className="fill-mark-faint" d="M625 520A185 185 0 0 1 995 520H865A55 55 0 0 0 755 520Z" />
      <rect className="fill-mark-faint" x="865" y="455" width="130" height="410" rx="65" />
    </>
  );
}

// The pack's `masters/wordmark-black.svg`, its baseline at y = 0.
const WORD =
  "M333.5 -545.9H231.45V-675.78H84.96V-545.9H9.77V-434.08H84.96V-146.97C84.96 -46.88 144.53 7.81 253.91 7.81C283.2 7.81 315.92 4.39 348.63 -5.37L327.64 -115.23C316.89 -112.3 292.48 -108.89 281.25 -108.89C245.61 -108.89 231.45 -125 231.45 -159.67V-434.08H333.5ZM645.53 10.74C772.48 10.74 859.39 -48.34 886.25 -139.16L753.44 -164.06C737.81 -122.07 699.24 -99.61 647.97 -99.61C574.24 -99.61 524.43 -145.51 521.02 -234.86H894.55V-275.88C894.55 -462.4 780.78 -552.73 638.2 -552.73C478.54 -552.73 376 -438.48 376 -270.02C376 -97.66 478.54 10.74 645.53 10.74ZM521.99 -326.66C527.85 -394.04 569.84 -442.38 639.67 -442.38C707.54 -442.38 746.11 -397.95 753.44 -326.66ZM1121.13 -727.54H974.65V0H1121.13ZM1225.64 0H1372.13V-330.08C1372.13 -395.51 1411.68 -431.64 1463.44 -431.64C1513.24 -431.64 1548.4 -397.46 1548.4 -344.24V0H1690V-335.94C1690 -393.07 1724.18 -431.64 1779.84 -431.64C1829.16 -431.64 1866.27 -401.86 1866.27 -340.33V0H2013.24V-368.16C2013.24 -486.82 1939.02 -554.2 1838.93 -554.2C1763.73 -554.2 1694.39 -515.62 1663.63 -431.64C1638.24 -514.65 1592.83 -554.2 1523.01 -554.2C1457.58 -554.2 1396.54 -519.53 1367.25 -439.94L1361.39 -545.9H1225.64ZM2362.38 10.74C2526.45 10.74 2629.47 -102.54 2629.47 -270.51C2629.47 -439.45 2526.45 -552.73 2362.38 -552.73C2197.83 -552.73 2095.29 -439.45 2095.29 -270.51C2095.29 -102.54 2197.83 10.74 2362.38 10.74ZM2362.38 -104.49C2284.26 -104.49 2244.71 -175.78 2244.71 -271C2244.71 -367.68 2284.26 -437.5 2362.38 -437.5C2440.51 -437.5 2480.55 -367.19 2480.55 -271C2480.55 -175.78 2440.51 -104.49 2362.38 -104.49ZM2858.01 -315.92C2858.01 -388.18 2901.46 -430.18 2963.96 -430.18C3025.49 -430.18 3062.6 -389.65 3062.6 -321.78V0H3209.08V-347.17C3209.08 -475.1 3134.86 -552.73 3021.58 -552.73C2937.6 -552.73 2882.42 -509.28 2851.66 -433.11L2849.71 -545.9H2711.52V0H2858.01ZM3314.08 0H3460.57V-545.9H3314.08ZM3387.32 -617.19C3431.27 -617.19 3467.89 -650.39 3467.89 -692.38C3467.89 -733.89 3431.27 -767.58 3387.32 -767.58C3342.89 -767.58 3306.76 -733.89 3306.76 -692.38C3306.76 -650.39 3342.89 -617.19 3387.32 -617.19Z";
