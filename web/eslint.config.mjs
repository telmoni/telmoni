
import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypeScript from "eslint-config-next/typescript";

const PALETTE_RE =
  "/(^|\\s|!|:)(bg|text|border|divide|ring|fill|stroke|outline|accent|caret|placeholder|from|via|to)-(white|black|(gray|zinc|slate|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-[0-9]+)(\\b|\\/)/";
const PALETTE_MSG =
  "Use semantic design tokens — surfaces (bg-card, bg-popover, text-muted-foreground) and sentiment (text-destructive, text-brand-positive, text-status-caution) — instead of raw Tailwind palette classes (grayscale OR chromatic). They break dark mode.";

const HEX_RE = "/#[0-9a-fA-F]{3,8}\\b/";
const HEX_MSG =
  "No raw hex colors — define/consume a semantic token instead (globals.css @theme; see AGENTS.md Frontend). Exception: app/global-error.tsx renders without the token stylesheet and is exempted via ignores.";

const RADIUS_RE = "/(^|\\s|!|:)rounded-\\[/";
const RADIUS_MSG =
  "No `rounded-[…]` — an arbitrary radius is a step nothing else in the skin can reach, and it is how a product ends up with six values that are all nearly 10px. Spend a scale step (rounded-xs … rounded-2xl, all derived from `--radius`), `rounded-full` for a disc or pill, or one of the named surface tokens in globals.css.";

const tokenSelectors = [
  {
    selector: `Literal[value=${PALETTE_RE}]`,
    message: PALETTE_MSG,
  },
  {
    selector: `TemplateElement[value.raw=${PALETTE_RE}]`,
    message: PALETTE_MSG,
  },
  {
    selector: `Literal[value=${HEX_RE}]`,
    message: HEX_MSG,
  },
  {
    selector: `TemplateElement[value.raw=${HEX_RE}]`,
    message: HEX_MSG,
  },
  {
    selector: `Literal[value=${RADIUS_RE}]`,
    message: RADIUS_MSG,
  },
  {
    selector: `TemplateElement[value.raw=${RADIUS_RE}]`,
    message: RADIUS_MSG,
  },
];

const bareFetchSelector = {
  selector: "CallExpression[callee.type='Identifier'][callee.name='fetch']",
  message:
    "Use fetchWithTimeout() from @/lib/api/fetch — bare fetch() has no timeout and parks the Next.js request slot if the upstream stalls.",
};

const dialogCloseSelector = {
  selector:
    "JSXOpeningElement[name.name=/^(DialogContent|SheetContent|CommandDialog)$/] > JSXAttribute[name.name='showCloseButton'] > JSXExpressionContainer > Literal[value=false]",
  message:
    "Don't hide a modal's close button — it removes the visible exit while Escape still closes it. If the choice must be forced, use AlertDialog instead.",
};

const eslintConfig = [
  ...nextCoreWebVitals,
  ...nextTypeScript,
  {
    rules: {
      "no-restricted-imports": [
        "error",
        {
          patterns: [
            {
              group: ["@mui/*"],
              message:
                "MUI is removed. Use components from @/components/ui (shadcn/ui) instead.",
            },
            {
              group: ["@emotion/*"],
              message:
                "Emotion is removed. Use Tailwind utility classes via @/lib/utils#cn.",
            },
          ],
        },
      ],
    },
  },
  {
    files: ["app/**/*.{ts,tsx}", "components/**/*.{ts,tsx}", "lib/**/*.{ts,tsx}"],
    ignores: ["components/ui/**", "**/*.test.{ts,tsx}", "app/global-error.tsx"],
    rules: {
      "no-restricted-syntax": ["error", dialogCloseSelector, ...tokenSelectors],
    },
  },
  {
    // ⚠ **`*actions` and not `actions` — the plain spelling is the minority.**
    // `app/**/actions.ts` matches only a file called exactly that, so
    // `notice-actions.ts`, `rename-actions.ts` and `search-actions.ts` sat
    // outside the bare-`fetch` ban: three Server Action
    // files, every one of them making outbound calls, exempt from the rule
    // written for outbound calls. `server-action-lint.test.ts` asks ESLint
    // itself whether each `"use server"` file is covered, so the next
    // spelling cannot slip out the same way.
    //
    // ⚠ **Every file that can run on the server, and not a list of the ones
    // that came to mind.** The list was seven globs and it still had holes —
    // `lib/analytics.ts` made its own call outside all of them. Server
    // Components under `app/`, everything under `lib/`, and the two root files
    // are in; `components/` is out because a fetch there runs in the browser,
    // where no request slot is parked and the wrapper's server imports cannot
    // load. `lib/api/fetch.ts` IS the wrapper and is the one file that may say
    // `fetch`.
    files: [
      "app/**/*.{ts,tsx}",
      // `.tsx` too: `lib/store/provider.tsx` is a `lib` file that the `.ts`
      // glob walked straight past, which is the same hole this list was
      // widened to close.
      "lib/**/*.{ts,tsx}",
      "proxy.ts",
      "instrumentation.ts",
    ],
    ignores: ["**/*.test.{ts,tsx}", "lib/api/fetch.ts", "app/global-error.tsx"],
    rules: {
      "no-restricted-syntax": ["error", bareFetchSelector, dialogCloseSelector, ...tokenSelectors],
    },
  },
  {
    ignores: [
      ".next/**",
      "node_modules/**",
      "next-env.d.ts",
    ],
  },
];

export default eslintConfig;
