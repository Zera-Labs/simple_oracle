#!/usr/bin/env sh
set -e

export ROCKET_ADDRESS=0.0.0.0
# Map Railway PORT to Rocket
if [ -n "${PORT}" ]; then
	export ROCKET_PORT="${PORT}"
fi

# Default DB path if not set
if [ -z "${ORACLE_DB_PATH}" ]; then
	export ORACLE_DB_PATH="/data/oracle.sqlite"
fi

exec /app/zera_oracle 