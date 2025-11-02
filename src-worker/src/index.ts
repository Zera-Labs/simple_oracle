export default {
  async fetch(request: Request, env: any, ctx: any): Promise<Response> {
    const url = new URL(request.url);

    if (!url.pathname.startsWith('/api/v1/qn/')) {
      return new Response('Not handled', { status: 404 });
    }

    const cache = (caches as any).default as Cache;
    const cacheKey = new Request(request.url, { method: 'GET' });

    const { maxAge, swr } = pickTtls(url.pathname);

    // Edge cache lookup
    const cached = await cache.match(cacheKey);
    if (cached) {
      // Serve immediately; refresh if stale
      const age = parseInt(cached.headers.get('Age') || '0', 10);
      if (Number.isFinite(age) && age > maxAge) {
        ctx.waitUntil(refresh(cache, cacheKey, request, env, maxAge, swr));
        return withSWRHeaders(cached, maxAge, swr, 'stale');
      }
      return withSWRHeaders(cached, maxAge, swr, 'hit');
    }

    // Miss -> fetch origin
    const res = await fetchOrigin(request, env.ORIGIN_URL);
    if (!res.ok) return res;
    const store = new Response(res.body, res);
    store.headers.set('Cache-Control', `public, max-age=${maxAge}, stale-while-revalidate=${swr}`);
    ctx.waitUntil(cache.put(cacheKey, store.clone()));
    return withSWRHeaders(store, maxAge, swr, 'miss');
  },
} as any;

function pickTtls(path: string): { maxAge: number; swr: number } {
  // Simple path-based TTLs to mirror origin adaptive TTL
  if (path.includes('/tokens/')) return { maxAge: 45, swr: 180 };
  if (path.includes('/pools/')) return { maxAge: 45, swr: 180 };
  if (path.includes('/dexes') || path.includes('/search')) return { maxAge: 300, swr: 300 };
  return { maxAge: 45, swr: 180 };
}

async function refresh(cache: Cache, key: Request, req: Request, env: any, maxAge: number, swr: number) {
  const res = await fetchOrigin(req, env.ORIGIN_URL);
  if (!res.ok) return;
  const copy = new Response(res.body, res);
  copy.headers.set('Cache-Control', `public, max-age=${maxAge}, stale-while-revalidate=${swr}`);
  await cache.put(key, copy);
}

function withSWRHeaders(res: Response, maxAge: number, swr: number, state: 'hit'|'stale'|'miss'): Response {
  const r = new Response(res.body, res);
  r.headers.set('Cache-Control', `public, max-age=${maxAge}, stale-while-revalidate=${swr}`);
  r.headers.set('X-Edge-Cache', state);
  return r;
}

async function fetchOrigin(request: Request, originBase: string): Promise<Response> {
  const inUrl = new URL(request.url);
  const base = new URL(originBase);
  inUrl.hostname = base.hostname;
  inUrl.protocol = base.protocol;
  inUrl.port = base.port;
  const upstream = new Request(inUrl.toString(), { method: 'GET', headers: request.headers });
  return fetch(upstream);
}


