# nat-matrix:netns NAT 行为矩阵 harness

五个网络命名空间(pub / gwA / gwB / a / b)里跑真实二进制
`mtdssd`(常驻守护进程,双侧各一)/ `mtdss`(CLI)/ `signaling-server` /
`stun-server`,用 iptables(conntrack / recent)模拟四种 NAT 行为,验证
打洞与传输。只能在 Linux 上跑。

每侧生命周期:CLI 经本机 IPC 登录(`mtdss login`,dev 验证码回环)→ daemon
常驻(信令在线循环 + 常驻接收)→ 发送侧 `mtdss send`(RPC 长调用,传输
编排全程在发送 daemon 内)→ 收端落盘由 daemon 常驻接收完成(旧「b 侧 recv
后台进程」已删)。

## 本机复跑(任意 Linux 盒子,需 root)

```bash
cargo build --release -p mtdss-cli -p mtdss-daemon -p signaling -p stun-server
sudo bash scripts/nat-matrix/matrix.sh
```

单例手动跑:

```bash
sudo bash scripts/nat-matrix/run_case.sh full full ok
sudo bash scripts/nat-matrix/run_case.sh symmetric symmetric fail
sudo bash scripts/nat-matrix/chaos.sh delay200
```

## 拓扑(TEST-NET-3,编址定稿)

```
ns pub:  203.0.113.1/30 (链 A 侧)  + 203.0.113.5/30 (链 B 侧)
ns gwA:  10.0.1.1/30 (lan) + 203.0.113.2/30 (ext)     ns a: 10.0.1.2/30, 默认路由 → gwA
ns gwB:  10.0.2.1/30 (lan) + 203.0.113.6/30 (ext)     ns b: 10.0.2.2/30, 默认路由 → gwB
```

pub 内 `signaling-server`(0.0.0.0:8787,TLS,通告 stun 203.0.113.1:3478,3479)
与 `stun-server`(:3478/:3479)。gw 两侧 `ip_forward=1`;veth 命名
`<ns>-ext` / `<ns>-lan`(pub 侧 `pub-a` / `pub-b`)。NAT 类型配方见
`lib.sh::nm_apply_nat_rules`(full_cone / restricted_cone / port_restricted /
symmetric,与 plan 逐字一致)。

## 脚本

| 脚本 | 作用 |
| --- | --- |
| `lib.sh` | 公共库:编址、二进制发现与 `nm_build`、NAT 配方、TLS、daemon 拉起(`nm_start_daemon`)、CLI 登录(抓码回填)、presence 轮询 |
| `setup.sh` | 建拓扑 + 两侧 NAT 规则(`NM_TYPE_A`/`NM_TYPE_B`,幂等) |
| `teardown.sh` | 幂等清场(逐表刷 + 删五个 ns) |
| `payload.sh` | 生成 4 文件 ×3MiB 载荷并输出 sha256 清单 |
| `run_case.sh` | 单例:`run_case.sh <typeA> <typeB> <ok\|fail>`;`NM_AUTO_SPAWN_A=1` 为 auto-spawn 门禁模式(见下);`NM_RELAY=1` 为中继兜底用例(见下) |
| `chaos.sh` | netem 混沌单例:`chaos.sh <loss10\|delay200\|blackout3\|permfail>`(Task 10,拓扑恒 port_restricted×port_restricted) |
| `matrix.sh` | 逐例跑全部用例并聚合退出码(14 矩阵例 + 1 auto-spawn 门禁例 + 1 中继兜底例 + 4 chaos 例) |

chaos 注入说明:delay200 用**固定 200ms**(无 ±50ms 抖动)。netem 的抖动是
每包独立抽值,QUIC 以 cwnd 突发发包,突发内必被重排、超 3 包丢包判定容限,
cubic 在 400ms RTT 下把吞吐自锁在初始窗口(实测 ~50KB/s,180s 送不完,
run 39013491209/39014880905/39016181767);固定延迟 tfifo 严格保序,保留
"延迟容差"用例本意。详见 `chaos.sh` 头注释。

## auto-spawn 门禁用例(`NM_AUTO_SPAWN_A=1`)

`run_case.sh full full ok` 的变体:a 侧 daemon 只临时在场供登录,登录后即
收掉——send 阶段 a 侧无 daemon,由 `mtdss send` 的 connect_or_spawn 自动
发现并后台拉起 `mtdssd`(同 home 同 `MTDSS_HOME`)。这是「CLI 自动拉起
daemon」路径唯一的真实二进制覆盖;断言仍为 ok 全链(落盘齐 + srflx 候选
+ 「接收完成」终局行,a 侧候选行改从 CLI 输出 grep——CLI 拉起的 daemon
无 `--log-file`,日志随继承的 stderr 落在 CLI 输出里)。

## 中继兜底用例(`NM_RELAY=1`)

`run_case.sh symmetric symmetric ok` 的变体:信令附中继数据面(`--relay-port
8788 --relay-addr 203.0.113.1`),账号 `dev@test` 经 `mtdss-admin` 开通中继并
限速 8192kbps(12MiB 默认载荷 ≈ 12s,矩阵时限内;均在 `lib.sh` 内以 `NM_RELAY=1`
门控)。对称×对称直连必穷尽,传输面自动走服务端中继兜底——客户端打洞穷尽
兜底路径唯一的真实二进制覆盖;断言在 ok 全链(落盘齐 + srflx 候选 + 「接收
完成」终局行)之外追加 daemon 日志「中继兜底已建立」锚行(确经中继,而非
直连侥幸成功)。门控只影响显式 `NM_RELAY=1` 的用例:其余 14 例不开中继,
symmetric fail 语义保持。拓扑注:中继 socket 绑 0.0.0.0(二进制硬编码),
gwB 侧回应源由 `NM_RELAY=1` 时落在 pub 的 SNAT 规则钉回通告地址(`lib.sh`
nm_start_servers 内注释详述——STUN 绑指定 IP 那条注释的同款问题的另一解法)。

## 环境变量

- `MTDSS_BIN_DIR` 覆盖二进制目录(默认 `<仓库根>/target/release`)
- `NM_RUST_LOG`(默认 info)、`NM_TYPE_A`/`NM_TYPE_B`(setup 两侧 NAT 类型)
- `NM_AUTO_SPAWN_A=1`(run_case auto-spawn 门禁模式,见上节)
- `NM_RELAY=1`(run_case 中继兜底用例:信令附中继数据面 + 账号开通中继,见上节)
- `NM_SEND_TIMEOUT_S` / `NM_REGISTER_TIMEOUT_S`(send / 登录硬超时)

## 测试专用环境变量(MTDSS_TEST_*)

仅 `mtdssd` 识别:daemon 启动时经 `apply_test_env` 一次性读取(注入经
`nm_start_daemon` 的额外 KEY=VALUE 传进 daemon 进程环境)。**测试专用、
不稳定、随时可删。**

| 变量 | 作用 |
| --- | --- |
| `MTDSS_TEST_IDLE_TIMEOUT_MS` | QUIC 空闲超时(毫秒;0 拒绝,保持默认) |
| `MTDSS_TEST_KEEPALIVE_MS` | QUIC keep-alive 间隔(毫秒;0 = 关闭) |
| `MTDSS_TEST_MAX_RECONNECTS` | 发送侧最大重连次数 |
| `MTDSS_TEST_BACKOFF_MS` | 重连退避基数(毫秒,指数 ×2) |
| `MTDSS_TEST_RECV_WINDOW_MS` | 接收侧断连后的接收窗口(常驻接收无窗口语义,当前仅编排选项保留) |
