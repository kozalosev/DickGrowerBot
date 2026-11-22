---
paths:
  - "src/help/en.html"
  - "src/help/fa.html"
  - "src/help/it.html"
  - "src/help/mod.rs"
  - "src/help/ru.html"
  - "src/help/zh.html"
  - "src/config/help.rs"
  - "src/handlers/help.rs"
---

## Optional: what the help mentions

```
HELP_AUCTION_BOT=DickAuctionBot  # the bot auctioning promo codes; unset or empty => not mentioned
HELP_AUCTION_START_PARAMETER=utm-dgb  # the `start` of the link to it; unset or empty => a bare link
DONATE_URL=https://team.sadbot.dev/donate?utm_source=DickGrowerBot  # unset or empty => no /donate
```

The help is rendered once, at startup, from `help::Context`. A paragraph about something that may
be absent sits inside `{{ if … }}`. `/support` and `/cleanup` follow `commands::CommandToggles`, the
same switches that hide those commands from the menu; the auctions paragraph follows
`HELP_AUCTION_BOT`; the shrink paragraph follows `DailyShrinkConfig::enabled`, the same switch that
hides `/mentions`; and the streak paragraph follows the perk, whose grades render as an empty
string — which `{{ if }}` takes as false — when `STREAK_BONUS_GRADES` is empty or `DISABLE_STREAK`
is set.

tinytemplate reads the context through `Serialize`, never through `Display` — `render` turns it
into a `serde_json::Value` before any formatter sees it. So a `Username` that must be shown with its
`@` is marked `#[serde_as(as = "DisplayFromStr")]`; the derived `Serialize` writes the bare name.

