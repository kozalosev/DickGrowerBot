[@DickGrowerBot](https://t.me/DickGrowerBot)

[![CI Build](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml/badge.svg?branch=main&event=push)](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml)

A game bot for group chats that let its users grow their virtual "dicks" every day for some random count of centimeters (including negative values) and compete with friends and other chat members.

Additional mechanics
--------------------
_(compared with some competitors)_

* **The Dick of the Day** daily contest to grow a randomly chosen dick for a bit more.
* A way to play the game without the necessity to add the bot into a group (via inline queries with a callback button).
* Import from _@pipisabot_ and _@kraft28_bot_ (not tested! help of its users is required).

### Soon
* dick battles (PvP)
* global monthly events

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
cargo sqlx prepare
```
