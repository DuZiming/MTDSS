# nat-matrix:netns NAT 行为矩阵 harness

五个网络命名空间(pub / gwA / gwB / a / b)里跑真实二进制
`mtdss` / `signaling-server` / `stun-server`,用 iptables(conntrack / recent)
模拟四种 NAT 行为,验证打洞与传输。只能在 Linux 上跑。

## 本机复跑(任意 Linux 盒子,需 root)

```bash
cargo build --release -p mtdss-cli -p signaling -p stun-server
sudo bash scripts/nat-matrix/matrix.sh
```

单例手动跑:

```bash
sudo bash scripts/nat-matrix/run_case.sh full full ok
sudo bash scripts/nat-matrix/run_case.sh symmetric symmetric fail
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
| `lib.sh` | 公共库:编址、二进制发现、NAT 配方、TLS、注册、presence 轮询 |
| `setup.sh` | 建拓扑 + 两侧 NAT 规则(`NM_TYPE_A`/`NM_TYPE_B`,幂等) |
| `teardown.sh` | 幂等清场(逐表刷 + 删五个 ns) |
| `payload.sh` | 生成 4 文件 ×3MiB 载荷并输出 sha256 清单 |
| `run_case.sh` | 单例:`run_case.sh <typeA> <typeB> <ok\|fail>` |
| `chaos.sh` | netem 混沌单例:`chaos.sh <loss10\|delay200\|blackout3\|permfail>`(Task 10,拓扑恒 port_restricted×port_restricted) |
| `matrix.sh` | 逐例跑全部用例并聚合退出码(14 矩阵例 + 4 chaos 例) |

chaos 注入说明:delay200 用**固定 200ms**(无 ±50ms 抖动)。netem 的抖动是
每包独立抽值,QUIC 以 cwnd 突发发包,突发内必被重排、超 3 包丢包判定容限,
cubic 在 400ms RTT 下把吞吐自锁在初始窗口(实测 ~50KB/s,180s 送不完,
run 39013491209/39014880905/39016181767);固定延迟 tfifo 严格保序,保留
"延迟容差"用例本意。详见 `chaos.sh` 头注释。

环境变量:`MTDSS_BIN_DIR` 覆盖二进制目录(默认 `<仓库根>/target/release`);
`NM_RUST_LOG`(默认 info)、`NM_TYPE_A`/`NM_TYPE_B`(setup 两侧 NAT 类型)。

## CLI 测试专用环境变量(MTDSS_TEST_*)

仅 `mtdss` 二进制识别,经 `apply_test_env` 调真实传输参数。**测试专用、
不稳定、随时可删。**

| 变量 | 作用 |
| --- | --- |
| `MTDSS_TEST_IDLE_TIMEOUT_MS` | QUIC 空闲超时(毫秒;0 拒绝,保持默认) |
| `MTDSS_TEST_KEEPALIVE_MS` | QUIC keep-alive 间隔(毫秒;0 = 关闭) |
| `MTDSS_TEST_MAX_RECONNECTS` | 发送侧最大重连次数 |
| `MTDSS_TEST_BACKOFF_MS` | 重连退避基数(毫秒,指数 ×2) |
| `MTDSS_TEST_RECV_WINDOW_MS` | 接收侧断连后的接收窗口(毫秒) |
