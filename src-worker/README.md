Zera Oracle Edge Worker
=======================

Purpose: cache `/api/v1/qn/*` at Cloudflare edge with stale-while-revalidate to cut origin and QuickNode costs.

Setup
-----
1) Install deps

```bash
cd src-worker
npm i # or pnpm i / yarn
```

2) Configure origin

- Set `ORIGIN_URL` in `wrangler.toml` to your deployed Rocket service base.
  Example: `https://zera-oracle.railway.app`

3) Dev and deploy

```bash
npm run dev    # local dev mode
npm run deploy # publish to Cloudflare
```

4) Route

- In Cloudflare dashboard, attach the Worker to `yourdomain.com/api/v1/qn/*`.

Local testing
--------------

```bash
# Run origin locally in another shell (from repo root):
cargo run

# In src-worker/, run the Worker locally and proxy to origin
npm run dev

# Test edge path
curl http://127.0.0.1:8787/api/v1/qn/addon/912/search?query=orca -i
```

How it works
------------
- Worker checks `caches.default` for a key = full URL.
- If fresh: returns immediately.
- If stale: serves stale and refreshes in background (`waitUntil`).
- If miss: proxies to origin, sets `Cache-Control: public, max-age=N, stale-while-revalidate=M` and stores in cache.
- TTLs are path-based: tokens/pools ~45s, dexes/search ~300s; SWR ~180–300s.

Notes
-----
- The origin already has singleflight, budgets, and an L2 cache; the Worker mostly ensures most requests never reach origin.
- If you need global singleflight across POPs, introduce a Durable Object as a lock—usually not necessary given origin protections.


