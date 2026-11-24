---
paths:
  - "src/handlers/pvp.rs"
  - "src/handlers/utils/locks.rs"
---

## The bet of an inline offer

An inline query asks for an offer with a number, alone (`@DickGrowerBot 100`) or after `pvp` or
any of its aliases, in any case (`@DickGrowerBot Pvp 100`, since a phone capitalizes the first
letter). The command alone (`@DickGrowerBot pvp`) offers `PVP_DEFAULT_BET`. The names come from
`BattleCommands`, so a new alias works here too. `inline_bet` is the
one parser, for the query and for the chosen result alike. An empty query lists every command, the
offer at `PVP_DEFAULT_BET` among them.

The offer's title in that list carries the bet near its start and stays short: a phone shows about
thirty characters of it, and a bet cut off there is why people kept asking how to change it.

## Optional: checking an inline PvP offer when it is posted

```
PVP_CHECK_INLINE_OFFER=true  # false => the initiator is checked only when the button is pressed
```

`/pvp` checks the initiator's length before it offers anything. An inline offer is built by
Telegram, so the earliest the bot can check it is `pvp_inline_chosen_handler`: if the initiator is
short of the bet, the posted message is edited into the `not_enough.initiator` notice, its button
goes, and it is scheduled as a `Notice` rather than an `Application`.

That is only possible where the chat can be decoded from the `inline_message_id` — a supergroup,
with `CHATS_MERGING_ENABLED` on. With merging off the battle itself keys the chat by
`chat_instance`, a different `Dicks` row than the decoded id names, so the check would look at the
wrong dick. Everywhere else the offer is posted unchecked, as before.

The check is one query per posted offer. Under high load, `false` drops it and leaves the check at
the press, which runs in every case anyway.

