import { createMDX } from "fumadocs-mdx/next";

const securityHeaders = [
  { key: "Strict-Transport-Security", value: "max-age=63072000; includeSubDomains; preload" },
  { key: "X-Frame-Options", value: "DENY" },
  { key: "X-Content-Type-Options", value: "nosniff" },
  { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
  { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=()" },
];

// The bare hostname of a dev tunnel, when one is up (`DEV_TUNNEL_HOST` in
// .env.local). `next dev` refuses cross-origin requests for its own assets, so
// a console reached at https://dev.example.com — the shape Slack's HTTPS-only
// redirect forces on a laptop — would render a page whose scripts 403 without
// this; and Server Actions refuse a request whose origin is not the host they
// are told they answer, which a tunnel that rewrites `Host` would make every
// action, so the host is named for them too. Unset means no entry, which is
// every deployed tier: there is no dev server there for the setting to mean
// anything to.
const devTunnelHost = process.env.DEV_TUNNEL_HOST?.trim();

// Typed, so a misspelt option is flagged where it is written. Strict Mode is
// the App Router's own default, and pino goes unbundled by Next's own list.
/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "standalone",
  poweredByHeader: false,
  // The root AGENTS.md is the repository's only instruction file; left on,
  // `next dev` re-creates web/AGENTS.md and web/CLAUDE.md whenever an agent
  // runs it.
  agentRules: false,
  // The rail's toggle sits at the bottom-left, where Next's dev badge floats;
  // the badge moves to the other corner so the toggle stays under the pointer.
  devIndicators: { position: "bottom-right" },
  ...(devTunnelHost ? { allowedDevOrigins: [devTunnelHost] } : {}),
  experimental: {
    // lucide-react is on Next's own list of packages it imports this way.
    optimizePackageImports: ["radix-ui"],
    ...(devTunnelHost ? { serverActions: { allowedOrigins: [devTunnelHost] } } : {}),
  },

  async headers() {
    return [
      {
        source: "/(.*)",
        headers: securityHeaders,
      },
    ];
  },

  async redirects() {
    return [
      // Old addresses of the book, which now lives at `/docs` (`content/docs`).
      { source: "/cookbook", destination: "/docs", permanent: false },
      { source: "/blueprints", destination: "/docs", permanent: false },
      { source: "/docs/cookbook", destination: "/docs", permanent: false },
      { source: "/docs/blueprints", destination: "/docs", permanent: false },

      // `/legal/*` is `proxy.ts`'s: it redirects to the deployment's own
      // documents at request time, which a build-time rule here could not.

      // The CLI's release workflow publishes the installer beside its
      // archives; this repository publishes no release at all.
      {
        source: "/install.sh",
        destination:
          "https://github.com/telmoni/telmoni-cli/releases/latest/download/install.sh",
        permanent: false,
      },
    ];
  },
};

// Fumadocs MDX compiles `content/docs` into pages; the Next plugin is how.
export default createMDX()(nextConfig);
