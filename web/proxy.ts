const EXCLUDED_ROOTS = new Set([
  'api',
  'auth',
  'invite',
  'connect',
  'cli',
  'console',
  'account',
  'legal',
  '_next',
  'favicon.ico',
]);

import { unsealData } from 'iron-session';
import { type NextProxy, type NextRequest, NextResponse } from 'next/server';

import { logger } from '@/lib/logger';
import { isPublic } from '@/lib/proxy/public-paths';
import { RESERVED_SLUGS, isValidSlug } from '@/lib/slug';

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

// The identity provider's origins, for `form-action`: sign-in and sign-out
// leave this site through redirects the browser holds to that directive, so
// a provider missing from it is a sign-in that dies at the first hop. Space
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

  const loggedIn     = await isLoggedIn(request);
  const isProtected  = !isPublic(pathname);

  if (pathname === '/organization' || pathname.startsWith('/organization/')) {
    const activeOrg = request.cookies.get('telmoni-active-organization')?.value;
    if (activeOrg && activeOrg !== 'organization' && (isValidSlug(activeOrg.toLowerCase()) || /^org_[0-9A-Za-z]+$/.test(activeOrg))) {
      const rest = pathname.slice('/organization'.length);
      const target = `/${activeOrg}${rest || ''}${request.nextUrl.search}`;
      return NextResponse.redirect(new URL(target, request.url), 307);
    }
  }

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

  const parts = pathname.split('/').filter(Boolean);
  const first = parts[0];
  let orgToSync: string | null = null;
  if (first && !EXCLUDED_ROOTS.has(first.toLowerCase()) && !RESERVED_SLUGS.has(first.toLowerCase())) {
    const isInternalOrgId = /^org_[0-9A-Za-z]+$/.test(first);
    const isSlug = isValidSlug(first.toLowerCase());
    if (isInternalOrgId || isSlug) {
      const canonical = isInternalOrgId ? first : first.toLowerCase();
      requestHeaders.set('x-telmoni-organization-id', canonical);
      requestHeaders.set('x-telmoni-org-id', canonical);
      orgToSync = canonical;
    }
  }

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  

  if (orgToSync) {
    const currentCookie = request.cookies.get('telmoni-active-organization')?.value;
    if (currentCookie !== orgToSync) {
      response.cookies.set('telmoni-active-organization', orgToSync, {
        httpOnly: true,
        sameSite: 'lax',
        path: '/',
        secure: process.env.NODE_ENV === 'production',
        maxAge: 60 * 60 * 24 * 30,
      });
    }
  }

  response.headers.set('content-security-policy', csp);
  return response;
};

export const config = {
  matcher: [
    '/((?!_next/static|_next/image|favicon.ico|icon.svg|apple-icon.png|opengraph-image.png).*)',
  ],
};
