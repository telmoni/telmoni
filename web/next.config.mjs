
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
// this. Unset means no entry, which is every deployed tier: there is no dev
// server there for the setting to mean anything to.
const devTunnelHost = process.env.DEV_TUNNEL_HOST?.trim();

const nextConfig = {
  output: "standalone",
  reactStrictMode: true,
  poweredByHeader: false,
  // The root AGENTS.md is the repository's only instruction file; left on,
  // `next dev` re-creates web/AGENTS.md and web/CLAUDE.md whenever an agent
  // runs it.
  agentRules: false,
  ...(devTunnelHost ? { allowedDevOrigins: [devTunnelHost] } : {}),
  serverExternalPackages: ["pino", "pino-pretty"],
  experimental: {
    optimizePackageImports: ["lucide-react", "radix-ui"],
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
      { source: "/org", destination: "/organization", permanent: true },
      { source: "/org/:path*", destination: "/organization/:path*", permanent: true },
      { source: "/cookbook", destination: "https://docs.telmoni.com", permanent: false },
      { source: "/blueprints", destination: "https://docs.telmoni.com", permanent: false },
      { source: "/docs/cookbook", destination: "https://docs.telmoni.com", permanent: false },
      { source: "/docs/blueprints", destination: "https://docs.telmoni.com", permanent: false },
      { source: "/docs", destination: "https://docs.telmoni.com", permanent: false },
      { source: "/docs/:path*", destination: "https://docs.telmoni.com/:path*", permanent: false },

      // `/legal/*` is `proxy.ts`'s: it redirects to the deployment's own
      // documents at request time, which a build-time rule here could not.

      {
        source: "/install.sh",
        destination:
          "https://github.com/telmoni/telmoni/releases/latest/download/install.sh",
        permanent: false,
      },
    ];
  },
};

export default nextConfig;
