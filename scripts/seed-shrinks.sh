#!/usr/bin/env bash
# Seeds Stale_Dick_Shrinks (and the stale Dicks that feed it) so each shrink-paging case can be
# eyeballed in a real chat.
#
#   Usage: scripts/seed-shrinks.sh [--wipe-history] [--history-only] <1|2|3|4> <telegram_chat_id>
#
#   1  no paging      — 1 day,  3 users            → no buttons at all
#   2  users only     — 1 day,  2 pages of users   → the ⬅️/➡️ row only
#   3  days only      — 3 days, 3 users            → the date row only
#   4  both           — 3 days, 3 pages of users   → both rows
#
# Page size is TOP_LIMIT. Scenario 4 spans three user pages rather than two, so the middle one can
# show ⬅️ and ➡️ together — the only place that combination appears.
#
# --wipe-history drops the chat's whole shrink log first, including rows for real players. Without
# it only this script's own rows are replaced, so leftover history from earlier runs still counts
# towards the day axis and can quietly turn scenario 1 into scenario 3. It touches the log only —
# nobody's length changes.
#
# By default today's shrinks come from the bot actually running: the seeded dicks are left stale so
# the startup run (SHRINK_RUN_ON_STARTUP=true) shrinks them and posts a notification. Only past days
# are inserted directly, since the job can only ever write `current_date`.
#
# --history-only writes today's rows directly instead and leaves the dicks freshly grown, so the
# whole history is testable in the inline command with no run and no restart. The dicks have to be
# fresh: a job run would otherwise try to insert the same (chat, uid, today) rows and abort the
# entire batch on the primary key. That also means this mode produces no notification.
#
# --clear removes the seeded users, their dicks and their shrink rows again. Worth doing when you
# are finished: the seeded dicks are ordinary rows, so until then they sit at the top of /top.
#
# Use a throwaway chat. The script's own rows are confined to a synthetic uid range, but the shrink
# job the default mode sets up is not: any real player in the target chat who is already stale gets
# shrunk for real, and that is not undone by re-running with another scenario.
set -euo pipefail

USAGE='usage: seed-shrinks.sh [--wipe-history] [--history-only] <1|2|3|4> <telegram_chat_id>
       seed-shrinks.sh --clear <telegram_chat_id>'

WIPE_HISTORY=false
HISTORY_ONLY=false
CLEAR=false
while [[ "${1:-}" == --* ]]; do
    case "$1" in
        --wipe-history) WIPE_HISTORY=true ;;
        --history-only) HISTORY_ONLY=true ;;
        --clear)        CLEAR=true ;;
        *) echo "unknown flag '$1'"$'\n'"$USAGE" >&2; exit 1 ;;
    esac
    shift
done

if $CLEAR; then
    TG_CHAT_ID="${1:?$USAGE}"
    echo "--clear: removing this script's users, dicks and shrink rows from $TG_CHAT_ID"
    docker exec -i "${POSTGRES_CONTAINER:-dickgrowerbot-postgresql}" \
        psql -U "${POSTGRES_USER:-dickgrowerbot}" -d "${POSTGRES_DB:-dickgrowerbotdb}" \
        -v ON_ERROR_STOP=1 -v tg_chat_id="$TG_CHAT_ID" -v uid_base=8000000000000000 <<'SQL'
BEGIN;
CREATE TEMP TABLE target ON COMMIT DROP AS
    SELECT id FROM Chats WHERE chat_id = :tg_chat_id;

DELETE FROM Stale_Dick_Shrinks
    WHERE chat_id IN (SELECT id FROM target) AND uid >= :uid_base;
DELETE FROM Dicks
    WHERE chat_id IN (SELECT id FROM target) AND uid >= :uid_base;
-- Users are global, so they only go once nothing anywhere still points at them -- clearing one
-- chat must not orphan rows another chat is still using.
DELETE FROM Users u
    WHERE u.uid >= :uid_base
      AND NOT EXISTS (SELECT 1 FROM Dicks d WHERE d.uid = u.uid);
COMMIT;

SELECT count(*) AS synthetic_users_left FROM Users WHERE uid >= :uid_base;
SQL
    exit 0
fi

SCENARIO="${1:?$USAGE}"
TG_CHAT_ID="${2:?$USAGE}"

CONTAINER="${POSTGRES_CONTAINER:-dickgrowerbot-postgresql}"
DB_USER="${POSTGRES_USER:-dickgrowerbot}"
DB_NAME="${POSTGRES_DB:-dickgrowerbotdb}"
TOP_LIMIT="${TOP_LIMIT:-10}"

# Synthetic uids live above Telegram's id ceiling so the cleanup below can never match a real
# player. The Bot API guarantees user ids fit in 52 bits (~4.5e15); real ones are 9-10 digits
# today, so anything merely "large" (10^9, say) would sweep up actual users.
UID_BASE=8000000000000000

case "$SCENARIO" in
  1) USERS=3;                       PAST_DAYS=0 ;;
  2) USERS=$((TOP_LIMIT + 5));      PAST_DAYS=0 ;;
  3) USERS=3;                       PAST_DAYS=2 ;;
  # Three user pages, not two: only a middle page shows ⬅️ and ➡️ at once.
  4) USERS=$((TOP_LIMIT * 2 + 5));  PAST_DAYS=2 ;;
  *) echo "unknown scenario '$SCENARIO' (expected 1-4)" >&2; exit 1 ;;
esac

echo "scenario $SCENARIO -> $USERS users, $PAST_DAYS past day(s) + today, page size $TOP_LIMIT"
$WIPE_HISTORY && echo "--wipe-history: clearing the chat's existing shrink log first"
$HISTORY_ONLY && echo "--history-only: seeding today directly, dicks left fresh (no run, no notification)"

docker exec -i "$CONTAINER" psql -U "$DB_USER" -d "$DB_NAME" \
  -v ON_ERROR_STOP=1 \
  -v tg_chat_id="$TG_CHAT_ID" \
  -v users="$USERS" \
  -v past_days="$PAST_DAYS" \
  -v uid_base="$UID_BASE" \
  -v wipe_history="$WIPE_HISTORY" \
  -v history_only="$HISTORY_ONLY" <<'SQL'
BEGIN;

INSERT INTO Chats (chat_id) VALUES (:tg_chat_id)
    ON CONFLICT (chat_id) DO NOTHING;

CREATE TEMP TABLE target ON COMMIT DROP AS
    SELECT id FROM Chats WHERE chat_id = :tg_chat_id;

-- Wipe only this script's own users, so re-running a different scenario starts clean while any
-- real players in the chat are left alone. With --wipe-history the log goes entirely, since a
-- single leftover row on another date silently adds a day to the navigation axis.
DELETE FROM Stale_Dick_Shrinks
    WHERE chat_id IN (SELECT id FROM target)
      AND (uid >= :uid_base OR :wipe_history);
DELETE FROM Dicks
    WHERE chat_id IN (SELECT id FROM target) AND uid >= :uid_base;

INSERT INTO Users (uid, name)
    SELECT :uid_base + n, 'ShrinkTest ' || lpad(n::text, 2, '0')
    FROM generate_series(1, :users) AS n
    ON CONFLICT (uid) DO UPDATE SET name = EXCLUDED.name;

-- Positive, and stale enough for the job to find them overdue -- unless we're seeding today
-- ourselves, in which case they stay fresh so a run can't collide with those rows. Lengths descend
-- with n purely so the rendered list has an obvious order.
INSERT INTO Dicks (uid, chat_id, length, updated_at)
    SELECT :uid_base + n, t.id, 500 - n * 10,
           current_timestamp - CASE WHEN :history_only THEN interval '0 days' ELSE interval '30 days' END
    FROM generate_series(1, :users) AS n, target t;

-- Day 0 is today: seeded here only in --history-only mode, since otherwise the job writes it.
-- The loss varies by day as well as by user, so paging between dates visibly rewrites the text --
-- otherwise every day renders identically and an edit is indistinguishable from a no-op.
INSERT INTO Stale_Dick_Shrinks (chat_id, uid, lost_length, created_at)
    SELECT t.id, :uid_base + n, 100 * d + (50 - n), current_date - d
    FROM generate_series(1, :users) AS n,
         generate_series(CASE WHEN :history_only THEN 0 ELSE 1 END, :past_days) AS d,
         target t;

COMMIT;

SELECT s.created_at AS day, count(*) AS users
    FROM Stale_Dick_Shrinks s
    JOIN Chats c ON c.id = s.chat_id
    WHERE c.chat_id = :tg_chat_id
    GROUP BY s.created_at
    ORDER BY s.created_at DESC;
SQL

if $HISTORY_ONLY; then
cat <<EOF

Seeded, today included. Nothing else to do — the inline 'shrinks' command reads this live, so just
run it in the chat. No restart, and no notification either (the dicks are fresh, so a run would
find nothing to shrink).
EOF
else
cat <<EOF

Seeded. Today's row is still missing on purpose — start the bot with

    SHRINK_RUN_ON_STARTUP=true

and the daily job will shrink those dicks immediately and post the notification, adding today to
the list above. Then use the inline 'shrinks' command in the chat to page through the history.
EOF
fi
