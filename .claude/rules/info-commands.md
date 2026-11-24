---
paths:
  - "src/handlers/info/mod.rs"
  - "src/handlers/info/reset"
  - "src/handlers/info/voucher"
  - "src/commands.rs"
  - "src/handlers/inline.rs"
  - "locales/en.yml"
  - "locales/fa.yml"
  - "locales/it.yml"
  - "locales/ru.yml"
  - "locales/zh-TW.yml"
  - "locales/zh.yml"
---

## Commands that only inform

`/voucher`, `/reset`, `/buy` and `/donate` answer with a fixed text and at most one URL button
(`handlers::info`). They exist so that questions asked every day get their answer from the bot. A
new command of the kind is one more variant of `InfoCommands` and an arm of `InfoResponse::build`.

Inline mode offers `voucher`, `buy` and `donate` too, as the last variants of `InlineCommand`,
posting the same answer as a `Notice`. Only the main commands are there, not the aliases. `buy` and
`donate` are left out of the list while their link isn't configured (`InlineCommand::is_available`),
and `DISABLE_CMD_<KEY>` removes a command from it as it does from the menu. `reset` isn't offered:
the real command will be for a chat's administrators, and an inline query carries no chat to check
them in.

A short text, a title and a line above a button, is an ordinary translation
(`commands.<key>.text`). A paragraph or more is an embedded HTML file per language, like the
privacy policy, under `handlers/info/<key>/`.

**An alias is a variant without a description**, never teloxide's `#[command(aliases = …)]`.
`bot_commands()` leaves `aliases` out, so `COMMAND_NAMES` would not know them and the topic gate
would let them through in a forbidden topic. An undescribed variant has an empty description,
which keeps it out of the menu. `InfoCommands::key` maps it to its main command, and the
translations, the counter and the switch all go by that key.

Which menu shows which command is `commands::PublishedCommands`, decided per `MenuScope`. `/voucher`
and `/reset` are about a chat's leaderboard, so only the group menus list them, and `/reset` only
the administrators' one, since resetting a chat is theirs to decide. In a private chat both, aliases
included, get the standard `errors.not_group_chat`, the same as the game commands:
`InfoCommands::is_for_groups` makes their branch let them through to that gate. In a group,
`/reset` answers its administrators only and tells everybody else so, counted as
`command_rejected_total{command="reset",reason="admins_only"}`. `DISABLE_CMD_<KEY>` hides one of them from the menu and also stops
the bot answering it, aliases included. `/buy` also leaves the menu when `HELP_AUCTION_BOT` is
unset, and `/donate` without `DONATE_URL`; both then answer with the standard
`errors.feature_disabled`. A `DONATE_URL` that isn't a URL, and a
`HELP_AUCTION_START_PARAMETER` that breaks Telegram's rule for it (`StartParameter`: 1 to 64 Latin
letters, digits, `_` or `-`), is logged and counts as unset.

**Clicks on the buttons are not counted here, because they can't be.** Telegram never tells a bot
that a URL button was pressed. The links say where the click came from instead: the `start` of the
link to the auction bot is `HELP_AUCTION_START_PARAMETER` (`utm-dgb` in production), and
`DONATE_URL` carries `?utm_source=DickGrowerBot` to `team.sadbot.dev/donate`, a Cloudflare Worker
that counts its visits. The bot counts invocations only,
`command_<key>_usage_total`.

