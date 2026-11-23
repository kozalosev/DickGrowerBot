---
paths:
  - "src/handlers/support.rs"
  - "src/bans.rs"
  - "src/repo/users.rs"
  - "src/handlers/mod.rs"
  - "src/main.rs"
  - "migrations/34_users-banned-until.sql"
  - "migrations/35_erase-user-function.sql"
  - "migrations/36_forbid-updates-of-banned-users.sql"
  - "migrations/40_notify-on-ban-changes.sql"
---

## Optional: /support and user bans

```
SUPPORT_CHAT_ID=-1001234567890  # where /support relays messages; unset => command hidden and disabled
BAN_LIST_REFRESH=15m       # the backstop behind the notification, see below
```

**The chat is checked once, at startup.** `check_support_chat` asks `getChat`, and in a group
`getChatMember` about the bot itself. A chat Telegram doesn't know, a group that was upgraded to a
supergroup, or a group where the bot may not post is treated as an unset id: `/support` is hidden
from the menu and the help, and the relay is off. Typed anyway, the command answers
`errors.feature_disabled` at once, rather than asking for a message it will refuse. The `error!` says why, with the new id after a
migration. The outcome is `startup_verification_error{kind="support_chat_unusable"}`, which the
`DickGrowerBotStartupVerificationFailed` alert in server-configs watches. It is a gauge rather than
a log rule because the state lasts until a restart, while a log rule would resolve a minute after
the line. A chat lost while the bot runs shows up only as failed relays.

A reply to one of the bot's 🆘 messages in the support chat is copied to the user it names —
`copy_message`, so the user sees the bot rather than whoever in the chat wrote it. Any participant
may answer, and every reply is delivered, so a discussion among the staff belongs outside a reply
to the ticket. A failed copy (the user blocked the bot, or never opened a private chat with it) is
reported back in the support chat as a reply to the answer.

The recipient is read from the **entity**, never from the text: the link right after `🆘 ` (UTF-16
offset 3) is one only the bot can have written, because the name and the request are HTML-escaped.
The `<code>` line with the id is for humans — a name or a request can contain newlines and a
plausible copy of it. The branch sits at the top of the dispatcher tree, with the relay itself:
it writes nothing for anybody, and a banned user is exactly the one who may need an answer. The
language of the `↩️` button below is read the same way, from the text right after that `<code>`,
falling back to English.

**The user may answer back, and the thread lives in Telegram, not here.** The copy carries a button
whose callback data is the id of the 🆘 message it answers (`support:<message id>`). Pressing it puts
the user's dialogue into `SupportCommandState::Replying(ticket)`; replying to the copy natively works
too, because a replied-to message comes with its keyboard, and the ticket is read off the button
(`support_answered_ticket`). Either way the user's text is relayed as an ordinary 🆘 message, sent as
a reply to the ticket — with `allow_sending_without_reply`, so a deleted ticket only loses the
threading — and a reply to *that* reaches the user like the first one did. So there is no table of
conversations: the one piece of state is a dialogue that already existed, and `erase_user` has
nothing new to clear. A ticket never expires; the rate limit is what bounds a talkative user. The
native reply sits above the ban gate with the rest of `/support`; the button sits with the other
callbacks below it, so a banned user who presses it gets the ban notice and still has the native
reply and `/support` itself.

`Users.banned_until` (migration 34) holds the ban; `NULL` means the user is not banned. It is set
only from outside the bot, with the SQL functions of migration 35 (`erase_user`, `ban_user`,
`unban_user`) — there is no self-service delete command, and there must not be one: a deletion
request is answered by hand. `erase_user` deletes every row the user owns but **keeps** the `Users`
row (empty name, reset `created_at`, future `banned_until`), which is why none of the missing
`ON DELETE CASCADE` foreign keys matter.

`bans::BanList` keeps the whole (tiny) list in memory, because `banned_until` is a **sync** function
in a dptree filter: every instance needs the list of its own whatever else exists, which is why this
one did not move into the shared store with the rest of #155. What it needed was not another place
to keep the list but a way to hear that it changed, and only the database can say so — nothing in
the bot ever writes a ban.

So migration 40 puts an `AFTER UPDATE OF banned_until` trigger on `Users` that notifies the `bans`
channel, and `spawn_listen_task` reloads on it through a `PgListener`. A trigger rather than a line
in each of the three admin functions: their bodies would have to be repeated in that migration and
kept in step for ever, and a ban applied by a plain `UPDATE` would still go unheard. `pg_notify`
is delivered on commit, so a rolled-back ban is never announced.

The timer stays behind it as the backstop — a notification sent while the listener is reconnecting
is heard by nobody — and so does SIGHUP. A listening connection can't serve queries and is held for
as long as it listens, so `establish_database_connection` builds the pool
`LISTENER_CONNECTIONS` larger and `DATABASE_MAX_CONNECTIONS` keeps meaning what an operator set it
to.

It sits on `Users` rather than `Dicks` for three reasons: the check is free there (`banned_until` is
on the row being updated, so there is nothing to look up), `Users` has exactly one production write
(`create_or_update`, `src/repo/users.rs:13`) and nothing bulk, and a trigger on `Dicks` would fire
during the chat merge's bulk insert (`src/repo/chats.rs:609`) and abort the whole merge. The error
carries SQLSTATE `GD3E1` (`handlers::BANNED_SQL_CODE`) with the ban's end date as its message, which
both call sites turn back into the `errors.banned` notice.

`checks::reject_banned_users()` sits in the middle of the dispatcher tree in `main.rs`. The order
there is load-bearing: **nothing above the gate may write a row for its sender.** Only `/help`,
`/privacy` and `/support` qualify. `/start` does not — a promo deeplink activates a code — and
neither does `/language`, which writes to user-service. The gate is an `Update`-level filter rather
than a `Message` one because the inline handler upserts a `Users` row too, and that upsert would
restore an erased name.

