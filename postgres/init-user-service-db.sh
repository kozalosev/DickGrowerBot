#!/bin/bash
# Runs once, only when the shared PostgreSQL data directory is first initialized (empty
# volume). Provisions a separate role and database for the bundled user-service so the
# bot and the user-service share one PostgreSQL server while keeping isolated databases.
#
# If your ./data volume already exists, this script will NOT run again — create the
# database manually once:
#   docker compose exec postgres \
#     psql -U "$POSTGRES_USER" -c "CREATE ROLE \"$USER_SERVICE_POSTGRES_USER\" WITH LOGIN PASSWORD '$USER_SERVICE_POSTGRES_PASSWORD';" \
#          -c "CREATE DATABASE \"$USER_SERVICE_POSTGRES_DB\" OWNER \"$USER_SERVICE_POSTGRES_USER\";"
set -e

# Nothing to do if the user-service database wasn't configured.
if [ -z "$USER_SERVICE_POSTGRES_DB" ]; then
  echo "USER_SERVICE_POSTGRES_DB not set; skipping user-service database provisioning."
  exit 0
fi

psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<-EOSQL
    CREATE ROLE "$USER_SERVICE_POSTGRES_USER" WITH LOGIN PASSWORD '$USER_SERVICE_POSTGRES_PASSWORD';
    CREATE DATABASE "$USER_SERVICE_POSTGRES_DB" OWNER "$USER_SERVICE_POSTGRES_USER";
EOSQL
