---
paths:
  - "src/topics.rs"
  - "src/handlers/topics.rs"
  - "src/domain/objects/topics.rs"
  - "src/handlers/mod.rs"
  - "src/main.rs"
---

## Optional: restricting the bot to forum topics

A forum's admins can confine the bot to chosen topics with `/topics` (issue #102), which answers
with an inline picker: allow or forbid the topic it was invoked in, or allow every topic again. The
allowlist lives in the same `Chats.settings` jsonb as the chat language, under `topics` — an
id-keyed set, `{"<topic id>": true}`, absent or empty meaning "every topic", which is the default.
Only the keys carry meaning; an object is used rather than an array because it merges and deletes
in one statement and dedupes for free.

```
CHAT_TOPICS_CACHE_TIME=1h     # optional TTL for the per-chat allowed-topics cache
BOT_ADMIN_CACHE_TIME=1h       # optional TTL for what is known about the bot's rights in a chat
```

`checks::reject_forbidden_topic()` sits at the very top of the dispatcher tree, above even the ban
gate, so **every** command is covered rather than the game ones only. Its twin,
`reject_forbidden_topic_callback()`, does the same for the buttons, above the other callback
branches: a keyboard outlives the message it came with, so without it the whole game could still be
played from a forbidden topic by tapping an older message. They may sit there because they
write nothing for the sender — the property the ban gate's ordering depends on.

`/topics` is registered *above* the gate, so a chat can't lock itself out of its own setting. That
placement is the whole exemption: the gate needs no special case for it, and the branch matches
every form of the command (`@username` suffix included) because `filter_bot_command` knows the
bot's names. Moving the branch below the gate would silently break this — `checks::test` pins it
down.

Four things keep the gate narrow: only real forums (`utils::is_forum` — a supergroup linked to a
channel puts a `message_thread_id` on discussion-thread messages too); only commands, not every
message (a notice on each one would be noisier than the bot the setting was meant to quiet); only
**our** commands, matched by name against `commands::COMMAND_NAMES` and by the `@username` Telegram's
menu appends, which has to be one of the bot's names — a group usually holds several bots, and answering for another one's command is the
noise this feature exists to remove; and fail-open on a database error.

Two limits are Telegram's, not ours, and both shape the design:

* **Topics have no names here.** There is no `getForumTopics`, and a name only ever arrives on the
  service message of a topic being created. Nothing is stored or shown for them: a list of `#42`
  labels says less than a count. So the picker speaks only about the topic it was opened in —
  whether the bot works there, plus how many topics it is confined to overall — and every topic is
  allowed or forbidden from inside itself. That is also why there is no "drop that other topic"
  button: it could not be labeled.
* **Inline mode can't be restricted.** An `InlineQuery` carries no thread, and the
  `inline_message_id` decoded in `handlers/utils/tghack.rs` holds only
  `(dc_id, peer, message_id, access_hash)` — a message's topic isn't derivable from its own id.
  That half is issue #76.

Because the daily-shrink broadcast replies to nothing, it names the topic outright
(`AllowedTopics::primary()`, the lowest allowed id — a jsonb object has no order to recover).
That also fixes a failure that predates the feature: a forum whose General topic is closed refuses
a message sent without a topic. The setup message needs none of this — it only ever goes to legacy
basic groups, which can't be forums.

