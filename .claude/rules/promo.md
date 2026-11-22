---
paths:
  - "src/handlers/promo.rs"
  - "src/handlers/start.rs"
  - "src/repo/promo.rs"
  - "src/rate_limit.rs"
  - "src/domain/primitives/promo.rs"
---

## How many guesses a promo code takes

The channel posts hints and readers guess, so a limit on promo codes has to leave room for that.
`/promo <code>`, its dialogue step and the `/start` deeplink each count as one attempt; the confirm
button doesn't, since it only follows an attempt already counted. All three go through
`handlers::try_activate`, which asks `RateLimiter::promo_exceeded` before anything else.

**The limit was read off the curve below.** Over the first weeks it was measured, 1380 users tried a
code within an hour, 22 of them reached 20 attempts, 11 reached 30, 2 reached 50 and none reached
100. `SPAM_LIMIT_PROMO_REQUESTS=30` over a `1h` window is therefore a point on that curve: it leaves
everyone but the far edge alone and still rules out brute force. Keep the window at an hour, the
curve's own, so that the two stay comparable; to move the limit, read where the tail breaks.

An attempt over it answers like the general limiter — the first `RATE_LIMIT_WARNINGS` with
`commands.promo.errors.too_often`, the rest with nothing — and is counted as `too_often`.

`promo_activation_attempts_total{outcome}` counts every attempt, a malformed code included.
`promo_attempt_users_total{reached}` is a `metrics::ThresholdCurve` fed by the limiter's own counter,
exactly as the `rate_limit_*_windows_total` curves are, so it costs no round trip of its own. It
counts a user once each time their count within the window lands on one of its thresholds, so it is
a survival curve: `reached="1"` is everyone who tried, the **median** is the highest threshold
reached by at least half of them, the **max** is the highest threshold reached at all, and the
**average** is all attempts over `reached="1"`. Only the threshold becomes a label, never the user.

Sharing the counter has three consequences. The curve keeps counting past the limit, so it still
shows how far a refused guesser goes. A malformed code is refused before the limiter, so it is in the
outcomes but not on the curve — a typo is not a guess. And, as with the general limiter, a limit of
0 switches the curve off with it: to measure without limiting, set the limit far above the last
threshold rather than to 0.

