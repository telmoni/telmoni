import { unsealData } from 'iron-session';
import { type NextProxy, type NextRequest, NextResponse } from 'next/server';

import { logger } from '@/lib/logger';
import {
  ORGANIZATION_HEADER,
  PATH_HEADER,
  namedOrganization,
  unescapedPath,
} from '@/lib/proxy/organization';
import { isPublic } from '@/lib/proxy/public-paths';

async function isLoggedIn(request: NextRequest): Promise<boolean> {
  const value = request.cookies.get('telmoni_session')?.value;
  if (!value) return false;
  const password = process.env.AUTH_SECRET;
  if (!password) {
    // Through pino, not `console`: Next 16 runs the proxy on the Node.js
    // runtime only (a `runtime` segment config here is a build error), so
    // the same subscriber the rest of the server uses is available. A
    // total-outage condition should not arrive at DEFAULT severity.
    logger.error('proxy: AUTH_SECRET is unset — no session can be verified');
    return false;
  }
  try {
    const data = await unsealData<{ userId?: string }>(value, { password });
    return !!data?.userId;
  } catch {
    return false;
  }
}

// The identity provider's origins, for `form-action`: signing out from the
// account menu is a form post that ends in a redirect to the provider, which
// the browser holds to that directive, so a provider missing from it is a
// sign-out that dies at the first hop (sign-in, and the other sign-out links,
// are links, which the directive does not govern). Space
// separated, and the console has no other opinion about who the provider is:
// auth mints the URLs, and this only lets the browser follow them.
function authProviderOrigins(): string[] {
  return (process.env.AUTH_PROVIDER_ORIGINS ?? '')
    .split(/\s+/)
    .map((origin) => origin.trim())
    .filter((origin) => origin.length > 0);
}

export function contentSecurityPolicy(nonce: string): string {
  const isDev = process.env.NODE_ENV !== 'production';
  const formAction = ["'self'", ...authProviderOrigins()].join(' ');
  return `
    default-src 'self';
    script-src 'self' 'nonce-${nonce}' ${isDev ? "'unsafe-eval'" : ''};
    style-src 'self' 'unsafe-inline';
    connect-src 'self'
      ${isDev ? 'ws://localhost:* http://localhost:*' : ''};
    frame-src 'none';
    img-src 'self' data: blob: https:;
    font-src 'self';
    object-src 'none';
    base-uri 'self';
    form-action ${formAction};
    frame-ancestors 'none';
  `
    .replace(/\s{2,}/g, ' ')
    .trim();
}

// The documents `/legal/<name>` once served, now published by the operator
// under `LEGAL_URL`. Two old names fold into the document that carried them.
const LEGAL_ALIASES: Record<string, string> = {
  dpa: 'privacy-policy',
  'acceptable-use-policy': 'terms-of-service',
};

// Where `/legal/*` goes, or `null` for a deployment that published nothing:
// then the path falls through to the router, which has no page for it and
// answers 404, rather than to a policy this operator never wrote.
export function legalRedirect(pathname: string): string | null {
  const base = process.env.LEGAL_URL?.trim().replace(/\/$/, '');
  if (!base) return null;
  const name = pathname.replace(/^\/legal\/?/, '').replace(/\/+$/, '');
  if (!name) return `${base}/`;
  return `${base}/${LEGAL_ALIASES[name] ?? name}/`;
}

export const proxy: NextProxy = async (request) => {
  const { pathname } = request.nextUrl;

  if (pathname === '/legal' || pathname.startsWith('/legal/')) {
    const target = legalRedirect(pathname);
    if (target) return NextResponse.redirect(target, 308);
  }

  const unescaped = unescapedPath(pathname);
  if (unescaped) {
    const url = new URL(unescaped + request.nextUrl.search, request.url);
    return NextResponse.redirect(url, 308);
  }

  const loggedIn     = await isLoggedIn(request);
  const isProtected  = !isPublic(pathname);

  if (isProtected && !loggedIn) {
    if (pathname.startsWith('/api/')) {
      return NextResponse.json({ error: 'unauthenticated' }, { status: 401 });
    }
    const entry = '/auth/login';
    const url = new URL(entry, request.url);
    url.searchParams.set('returnTo', pathname + request.nextUrl.search);
    return NextResponse.redirect(url);
  }

  const nonce = btoa(crypto.randomUUID());
  const csp = contentSecurityPolicy(nonce);
  const requestHeaders = new Headers(request.headers);
  requestHeaders.set('x-nonce', nonce);
  requestHeaders.set('content-security-policy', csp);

  // The path names the organization the request acts in (`lib/slug.ts`), so
  // a page and the actions posted from it can never act in another one. The
  // path itself goes too: a layout redirects from it, and is handed only its
  // params. Next has already taken its own query parameters off it.
  requestHeaders.delete(ORGANIZATION_HEADER);
  requestHeaders.set(PATH_HEADER, pathname + request.nextUrl.search);
  const organization = namedOrganization(pathname);
  if (organization) requestHeaders.set(ORGANIZATION_HEADER, organization);

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set('content-security-policy', csp);
  return response;
};

export const config = {
  matcher: [
    '/((?!_next/static|_next/image|favicon.ico|icon.svg|apple-icon.png|opengraph-image.png).*)',
  ],
};
