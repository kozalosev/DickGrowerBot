---
paths:
  - "src/domain/objects/bot_names.rs"
  - "src/handlers/utils/command_filter.rs"
  - "src/handlers/utils/mod.rs"
  - "src/domain/primitives/username.rs"
  - "src/handlers/mod.rs"
  - "clippy.toml"
---

## The bot's usernames

A bot may hold several usernames (`@DickGrowerBot`, `@DickGrowerGame`, …), and a command addressed
to any of them is ours. `getMe` returns only the first, so `handlers::utils::fetch_bot_names` asks
`getChat` about the bot itself, whose `active_usernames` lists them all. It does so once, at
startup: a username added later starts working after a restart. If the call fails, the bot answers
to `getMe`'s name alone and logs a warning.

`domain::objects::BotNames` is a dependency of the dispatcher, and both places that tell our commands apart use it:
the command branches, through `filter_bot_command`, and `checks::is_our_command`, which serves the
rate limiter, the topics gate, the title keeper and the span-skipping in `trace_updates`. teloxide's
own `filter_command` passes `getMe`'s name to the parser, so it would refuse the others;
`clippy.toml` forbids it.

## How a player's name is shown

Every name the bot prints goes through `Username::escaped()`, and that is where a link in it is
defused: spammers put URLs into their display names, and Telegram would make them clickable in the
bot's message. Each link becomes the custom 🚫 emoji (a `<tg-emoji>`, which needs the bot to own a
Fragment username — it does).

The links are found by `linkify`, **after** the invisible `Format` characters are stripped, so a
zero-width space can't hide `t.me` from the check while Telegram still links it. A link with a
scheme always counts. A bare domain counts only if its TLD is in `KNOWN_TLDS`: `linkify` takes any
two letters for one, and `Mr.Smith` is a name, not a link. That list is what to extend when a spam
domain gets through. `linkify` never finds a bare domain under a non-ASCII TLD (`сайт.рф`), so the text
between its links goes through `IDN_LINK_REGEX` too, built from `KNOWN_IDN_TLDS`. That one runs on
the escaped text, which is safe because escaping changes nothing a domain is made of.

