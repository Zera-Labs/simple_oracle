# Zera Devnet Mock Oracle (Rocket + SQLite)

A lightweight, centralized price oracle for Devnet usage. Admins can set/get prices for SPL/Token-2022 mints, with audit logging, JWT-protected writes, Server-Sent Events for live updates, and an optional price pegger.

- Base URL: `/api/v1`
- Admin UI: `/api/v1/admin`
- SSE stream: `/api/v1/sse`

## Features

- Integer price model: `usd_mantissa` (string, u128-safe) and `usd_scale` (u32)
- CRUD for prices, symbol map, and config
- JWT-based admin writes; public reads
- Rate-limited writes per admin
- Audit log with before/after snapshots
- SSE updates for clients
- Optional HTTP-JSON pegger via env configuration
- Simple SSH-friendly TUI for remote admin

## Endpoints (summary)

- GET `/health`
- GET `/prices` | GET `/prices/:mint`
- POST `/prices` | PATCH `/prices/:mint` | DELETE `/prices/:mint` (admin)
- GET `/symbols` | POST `/symbols` (admin)
- GET `/config` | PATCH `/config` (admin)
- GET `/audit?limit=100&cursor=...`
- GET `/sse` (Server-Sent Events)
- GET `/admin` (embedded admin web UI)
- POST `/admin/login` (issue JWT for UI/TUI)

## Environment Variables

- `ORACLE_NETWORK` (default: `devnet`)
- `JWT_SECRET` (required for admin login)
- `ADMIN_UI_PASSWORD` (required for `/admin` login)
- `DEFAULT_FEE_BPS` (default: `100`)
- `ZERA_MINT` (optional hint)
- `SUPPORTED_MINTS` (comma-separated list)
- `ORACLE_DB_PATH` (default: `./oracle.sqlite` or `/data/oracle.sqlite` in Docker)
- `WRITE_RATE_LIMIT_PER_MINUTE` (default: `60`)
- `USDC_DEVNET_MINT`, `ZERA_DEVNET_MINT` (optional seed fixtures)
- `PEG_SOURCES` (optional pegger; see Pegger below)
- Rocket network (usually set automatically in Docker/Railway):
  - `ROCKET_ADDRESS=0.0.0.0`
  - `ROCKET_PORT=<PORT>`

### Example .env (local)

```dotenv
RUST_LOG=info
ORACLE_NETWORK=devnet
JWT_SECRET=dev-secret
ADMIN_UI_PASSWORD=changeme
DEFAULT_FEE_BPS=100
ZERA_MINT=3ZaR...zeraMint...
SUPPORTED_MINTS=GkN1...usdcMint...
USDC_DEVNET_MINT=GkN1...usdcMint...
ZERA_DEVNET_MINT=3ZaR...zeraMint...
WRITE_RATE_LIMIT_PER_MINUTE=60
# PEG_SOURCES format: mint|url|json.path.to.price|scale  ;  multiple sources separated by semicolons
# PEG_SOURCES=3ZaR...|https://api.example.com/zera|data.price|2;GkN1...|https://api.example.com/usdc|data.price|2
```

## Run Locally

```bash
# 1) Setup env
cp README.md .env  # or create a .env with values above
# 2) Start server
cargo run
# 3) Health check
curl http://127.0.0.1:8000/api/v1/health
# 4) Open admin UI
xdg-open http://127.0.0.1:8000/api/v1/admin || open http://127.0.0.1:8000/api/v1/admin
```

Admin login uses `ADMIN_UI_PASSWORD`, issues a JWT, and allows managing prices.

## Docker

Build & run (SQLite persisted to `./data`):

```bash
docker build -t zera_oracle .
mkdir -p ./data

docker run --rm -p 8000:8000 \
  -e ROCKET_ADDRESS=0.0.0.0 \
  -e JWT_SECRET=dev-secret \
  -e ADMIN_UI_PASSWORD=changeme \
  -e ORACLE_DB_PATH=/data/oracle.sqlite \
  -v "$(pwd)/data:/data" \
  zera_oracle
```

Open `http://localhost:8000/api/v1/admin` and login.

## Railway Deployment

This repo includes a Dockerfile; Railway will auto-detect and build it.

- Required variables:
  - `ROCKET_ADDRESS=0.0.0.0`
  - `JWT_SECRET=<secret>`
  - `ADMIN_UI_PASSWORD=<password>`
  - `ORACLE_DB_PATH=/data/oracle.sqlite`
- Optional:
  - `WRITE_RATE_LIMIT_PER_MINUTE=60`
  - `PEG_SOURCES=...` (see Pegger)
  - `USDC_DEVNET_MINT`, `ZERA_DEVNET_MINT`
- Volume:
  - Create a Railway Volume and mount at `/data` for DB persistence.
- Networking:
  - Generate a public domain under Service → Networking.
  - The entrypoint maps Railway `PORT` to Rocket `ROCKET_PORT` automatically and binds `0.0.0.0`.
- URLs:
  - API base: `https://<your-domain>/api/v1`
  - Admin UI: `https://<your-domain>/api/v1/admin`

### Remote access / SSH on Railway

There are two practical management options:

1) From your machine (recommended):

```bash
# Interactively manage prices over HTTPS
cargo run --bin zera_oracle_tui -- --base https://<your-domain> --user ops
# You will be prompted for ADMIN_UI_PASSWORD; changes are audit-logged.
```

2) Inside the running container (if Railway shell/SSH is available in your plan):

- Open a shell via Railway UI or CLI (Metal/SSH). Then run:

```bash
# Inside container
/app/zera_oracle_tui --base http://127.0.0.1:8000 --user ops
```

Note: Availability of an interactive shell depends on your Railway plan and service settings. As an alternative, always use the TUI locally against the public domain or use the Admin UI.

## Pegger (auto-price updates)

Enable by setting `PEG_SOURCES`. The worker polls every ~15s and upserts prices with `updated_by="pegger"`.

Format per source: `mint|url|json.path.to.price|scale`

- `mint`: base58 mint
- `url`: HTTP endpoint returning JSON
- `json.path.to.price`: dot-separated path to a numeric price field in the JSON
- `scale`: integer USD scale used to derive `usd_mantissa` from the numeric price

Examples:

```dotenv
PEG_SOURCES=3ZaR...|https://api.mainnet.example.com/zera|data.price|2;GkN1...|https://api.mainnet.example.com/usdc|data.price|2
```

## API Examples

```bash
# Read all prices
curl https://<domain>/api/v1/prices

# Admin login -> JWT
token=$(curl -s -X POST https://<domain>/api/v1/admin/login \
  -H 'Content-Type: application/json' \
  -d '{"user":"ops","password":"changeme"}' | jq -r .token)

# Upsert a price
curl -X POST https://<domain>/api/v1/prices \
  -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  -d '{"mint":"3ZaR...","symbol":"ZERA","usd_mantissa":"10","usd_scale":2,"decimals":6}'

# Patch a price
curl -X PATCH https://<domain>/api/v1/prices/3ZaR... \
  -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
  -d '{"usd_mantissa":"8"}'

# Delete a price
curl -X DELETE https://<domain>/api/v1/prices/3ZaR... -H "Authorization: Bearer $token"
```

## Backups

- SQLite file is at `ORACLE_DB_PATH` (default `/data/oracle.sqlite` in Docker/Railway)
- Periodically copy or dump:

```bash
sqlite3 /data/oracle.sqlite ".backup '/data/oracle-$(date +%F).sqlite'"
# or
sqlite3 /data/oracle.sqlite ".dump" > /data/oracle_dump.sql
```

## Notes

- This is a mock oracle for Devnet. Treat it as centralized and for convenience only.
- For on-chain mirroring or Solana-native price pegs (e.g., Pyth/Switchboard), extend the pegger to query Solana RPC and derive prices on-chain. 