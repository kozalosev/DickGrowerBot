[@DickGrowerBot](https://t.me/DickGrowerBot)
============================================

[![CI Build](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml/badge.svg?branch=main&event=push)](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml) [![@DickGrowerBot MAU](https://tgbotmau.quoi.dev/api/bot/DickGrowerBot/mau/badge?style=flat "@DickGrowerBot MAU")](https://tgbotmau.quoi.dev/?bot=DickGrowerBot)

A game bot for group chats that let its users grow their virtual "dicks" every day for some random count of centimeters (including negative values) and compete with friends and other chat members.

Additional mechanics
--------------------
_(compared with some competitors)_

* **The Dick of the Day** daily contest to grow a randomly chosen dick for a bit more.
* A way to play the game without the necessity to add the bot into a group (via inline queries with a callback button).
* Import from _@pipisabot_ and _@kraft28_bot_ (not tested! help of its users is required).
* PvP fights with statistics.

### Soon (but not very, I guess)
* an option to show mercy and return the award for the battle back;
* support for those who loses battles the most;
* more perks;
* achievements;
* referral promo codes;
* global monthly events;
* a shop.

Features
--------
* true system random from the environment's chaos by usage of the `get_random()` syscall (`BCryptGenRandom` on Windows, or other alternatives on different OSes);
* English and Russian translations;
* Prometheus-like metrics.

Technical stuff
---------------

### Requirements to run
* PostgreSQL;
* _\[optional]_ Docker (it makes the configuration a lot easier);
* _\[for webhook mode]_ a frontal proxy server with TLS support ([nginx-proxy](https://github.com/nginx-proxy/nginx-proxy), for example).

### How to rebuild .sqlx queries?
_(to build the application without a running RDBMS)_

```shell
cargo sqlx prepare -- --tests
```

### Adjustment hints

It's most probably you want to change the value of the `GROW_SHRINK_RATIO` environment variable to make the players upset and disappointed more or less often.

### How to disable a command?

Most of the command can be hidden from both lists: command hints and inline results. To do so, specify an environment variable like `DISABLE_CMD_STATS` (where `STATS` is a command key) with any value.
Don't forget to pass this variable to the container by adding it to the `docker-compose.yml` file!
