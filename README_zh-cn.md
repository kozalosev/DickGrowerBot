[@DickGrowerBot](https://t.me/DickGrowerBot)
============================================

[![CI构建](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml/badge.svg?branch=main&event=push)](https://github.com/kozalosev/DickGrowerBot/actions/workflows/ci-build.yaml)

一款用于群聊的游戏机器人，让用户每天随机增长虚拟 "小鸡鸡 "的厘米数（包括负值），并与好友和其他聊天成员一较高下。

附加机制
--------------------
_（与某些竞争对手相比）_

* 每天**的 "小弟弟 "**比赛，让随机选择的 "小弟弟 "长得更大一些。
* 无需将机器人添加到群组即可进行游戏的方法（通过带回调按钮的内联查询）。
* 从 _@pipisabot_ 和 _@kraft28_bot_ 导入（尚未测试！需要用户帮助）。
* PvP 战斗统计。

#### 很快（但我猜不会很快）
* 可以选择手下留情并返还战斗奖励；
* 支持输得最惨的玩家；
* 更多福利；
* 成就；
* 推荐促销代码；
* 全球每月活动；
* 商店。

功能
--------
* 通过使用 `get_random()` 系统调用（Windows 上的 `BCryptGenRandom`，或不同操作系统上的其他替代方法），从环境的混乱中获得真正的系统随机；
* 英语和俄语翻译；
* 类似普罗米修斯的度量标准。

技术资料
---------------

### 运行要求
* PostgreSQL；
* _\[optional]_ Docker（它能让配置变得更简单）；
* _\[for webhook mode]_ 一个支持TLS的前端代理服务器（例如[nginx-proxy](https://github.com/nginx-proxy/nginx-proxy)）。

#### 如何重建 .sqlx 查询？
在没有运行 RDBMS 的情况下构建应用程序）_

```shell
cargo sqlx prepare -- --tests
```

### 调整提示

你很可能想改变 `GROW_SHRINK_RATIO` 环境变量的值，让玩家更多或更少感到不安和失望。

### 如何禁用命令？

大多数命令都可以从两个列表中隐藏：命令提示和内联结果。 为此，请指定一个环境变量，如 `DISABLE_CMD_STATS`（其中 `STATS` 是命令关键字），变量值不限。
别忘了把这个变量添加到 `docker-compose.yml` 文件中，以便传递给容器！
