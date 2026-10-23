#!/usr/bin/env bash
# lib.sh — netns NAT 矩阵公共库:编址常量、二进制发现与构建、NAT 规则配方、
# 测试网 TLS、daemon 拉起、设备登录(CLI login 抓码回填)、presence 轮询等。
# 被 setup.sh / teardown.sh / run_case.sh / chaos.sh source。
# 拓扑编址与 NAT 规则集为 plan 定稿(Task 8),只调机制、不改语义。

# ---------- 编址(TEST-NET-3,plan 定稿) ----------
NM_NS_PUB=pub
NM_NS_A=a
NM_NS_GWA=gwA
NM_NS_B=b
NM_NS_GWB=gwB
NM_PUB_IP_A=203.0.113.1 # pub 朝链 A 侧(信令/STUN 落这里)
NM_PUB_IP_B=203.0.113.5 # pub 朝链 B 侧
NM_GWA_EXT_IP=203.0.113.2
NM_GWB_EXT_IP=203.0.113.6
NM_A_LAN_IP=10.0.1.2
NM_B_LAN_IP=10.0.2.2
NM_A_LAN_GW=10.0.1.1
NM_B_LAN_GW=10.0.2.1
NM_SIG_PORT=8787
NM_STUN_PORT=3478
NM_STUN_ALT=3479
# 服务地址默认按定稿编址;环境可覆盖(Task 10 chaos 亦复用)
NM_SERVER_URL="${NM_SERVER_URL:-https://$NM_PUB_IP_A:$NM_SIG_PORT}"

# ---------- 二进制发现 ----------
NM_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [ -n "${MTDSS_BIN_DIR:-}" ]; then
  NM_BIN_DIR="$MTDSS_BIN_DIR"
else
  NM_BIN_DIR="$NM_REPO_ROOT/target/release"
fi
MTDSS_BIN="$NM_BIN_DIR/mtdss"
MTDSSD_BIN="$NM_BIN_DIR/mtdssd"
SIGNALING_BIN="$NM_BIN_DIR/signaling-server"
MTDSS_ADMIN_BIN="$NM_BIN_DIR/mtdss-admin"
STUN_BIN="$NM_BIN_DIR/stun-server"

# 构建全部四个二进制(CI 与本机复跑共用;矩阵只跑 release)
nm_build() {
  (cd "$NM_REPO_ROOT" && cargo build --release \
    -p mtdss-cli -p mtdss-daemon -p signaling -p stun-server)
}

# ---------- 小工具 ----------
nm_log() { printf '[nat-matrix] %s\n' "$*"; }
nm_die() { nm_log "ERROR: $*" >&2; exit 1; }

# 运行前置:root + 关键命令 + 四个 release 二进制
nm_require_env() {
  [ "$(id -u)" -eq 0 ] || nm_die "需要 root(CI 用 sudo 跑;本机 Linux 复跑: sudo bash scripts/nat-matrix/matrix.sh)"
  local c
  for c in ip iptables sysctl timeout dd sha256sum openssl curl mkfifo seq sort xargs python3; do
    command -v "$c" >/dev/null 2>&1 || nm_die "缺命令: $c"
  done
  [ -x "$MTDSS_BIN" ] || nm_die "找不到 $MTDSS_BIN(先: nm_build,或 cargo build --release -p mtdss-cli -p mtdss-daemon -p signaling -p stun-server)"
  [ -x "$MTDSSD_BIN" ] || nm_die "找不到 $MTDSSD_BIN(先: nm_build,或 cargo build --release -p mtdss-daemon)"
  [ -x "$SIGNALING_BIN" ] || nm_die "找不到 $SIGNALING_BIN"
  [ -x "$MTDSS_ADMIN_BIN" ] || nm_die "找不到 $MTDSS_ADMIN_BIN(与 signaling-server 同包:-p signaling)"
  [ -x "$STUN_BIN" ] || nm_die "找不到 $STUN_BIN"
}

# 在 ns 里执行命令
nexec() { ip netns exec "$1" "${@:2}"; }

# ns 是否存在(幂等清场/失败诊断共用)
nm_ns_exists() { ip netns list 2>/dev/null | awk '{print $1}' | grep -qx "$1"; }

# 轻量 STUN 可达探针(python3,无连接 UDP——与二进制 gather 同型):
# 发 20 字节 Binding Request(RFC5389 头),收 Binding Success(前 4 字节
# 01 01 00 0c)。每 try 打印本地端口/回应源,便于与 tcpdump 对账。
# 用法: nm_stun_probe <ns> <port> [tries=10];重试间隔 0.5s。
nm_stun_probe() {
  local ns="$1" port="$2" tries="${3:-10}"
  nexec "$ns" python3 - "$NM_PUB_IP_A" "$port" "$tries" <<'NM_STUN_PROBE'
import socket, sys, time
srv_ip, srv_port, tries = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
req = bytes([0, 1, 0, 0, 0x21, 0x12, 0xA4, 0x42]) + bytes(range(1, 13))
for i in range(tries):
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.settimeout(1.0)
    try:
        s.sendto(req, (srv_ip, srv_port))
        data, src = s.recvfrom(1500)
        print(f"probe try={i} local_port={s.getsockname()[1]} src={src[0]}:{src[1]} hex={data[:4].hex()}", flush=True)
        if data[:4].hex() == "0101000c":
            sys.exit(0)
    except (socket.timeout, OSError):
        # Linux 无连接 socket 对 ICMP 静默(超时重试);OSError 兜底其他平台
        print(f"probe try={i} local_port={s.getsockname()[1]} TIMEOUT", flush=True)
    finally:
        s.close()
    time.sleep(0.5)
sys.exit(1)
NM_STUN_PROBE
}

# 失败诊断:转储三个 ns 的 iptables 计数(nat/mangle/filter FORWARD)与
# UDP conntrack 条目,定位丢包发生在哪一跳。teardown 之前调用。
nm_dump_netns_state() {
  local ns
  for ns in "$NM_NS_GWA" "$NM_NS_GWB" "$NM_NS_PUB"; do
    nm_ns_exists "$ns" || continue
    echo "===== $ns: iptables 计数 + conntrack(udp/347) ====="
    nexec "$ns" iptables -v -n -L FORWARD 2>/dev/null || true
    nexec "$ns" iptables -t nat -v -n -L 2>/dev/null || true
    nexec "$ns" iptables -t mangle -v -n -L FORWARD 2>/dev/null || true
    nexec "$ns" sh -c "grep -E 'udp|3478|3479' /proc/net/netfilter/nf_conntrack 2>/dev/null | head -40" || true
  done
}

# NAT 类型记号归一:matrix 短名(full/restricted)与配方长名(full_cone/…)都收
nm_norm_type() {
  case "$1" in
    full | full_cone) echo full_cone ;;
    restricted | restricted_cone) echo restricted_cone ;;
    port_restricted) echo port_restricted ;;
    symmetric) echo symmetric ;;
    *) return 1 ;;
  esac
}

# ---------- NAT 规则配方(plan 逐字落地;<ext_if>=朝 pub 的 veth) ----------
# 用法: nm_apply_nat_rules <ns> <ext_if> <lan_if> <ext_ip> <peer_lan_ip> <type(已归一)>
nm_apply_nat_rules() {
  local ns="$1" ext_if="$2" lan_if="$3" ext_ip="$4" peer_lan_ip="$5" type="$6"
  case "$type" in
    full_cone | restricted_cone | port_restricted | symmetric) ;;
    *) nm_die "unknown NAT type: $type" ;;
  esac
  # 公共:固定外地址 SNAT + 转发放行出向。SNAT 端口策略按类型分档:
  # 锥形保端口(srflx 端口=本源端口,映射可预测);对称加 --random-fully
  # 按目的分配随机外部端口——同一源 socket 查 3478/3479 两个目的得到不同
  # 映射,客户端 nat_symmetric 检测(两次绑定查询比对)才判 true(plan
  # 定稿语义"不同目的 → 不同外部端口";保端口 SNAT 下两查同口会被误判
  # 非对称,Task 8 评审修正)。
  case "$type" in
    symmetric)
      nexec "$ns" iptables -t nat -A POSTROUTING -o "$ext_if" \
        -j SNAT --to-source "$ext_ip" --random-fully
      ;;
    *)
      nexec "$ns" iptables -t nat -A POSTROUTING -o "$ext_if" -j SNAT --to-source "$ext_ip"
      ;;
  esac
  nexec "$ns" iptables -A FORWARD -i "$lan_if" -o "$ext_if" -j ACCEPT

  case "$type" in
    full_cone) # 静态 DNAT(全端口) + 入向全放
      nexec "$ns" iptables -t nat -A PREROUTING -i "$ext_if" -j DNAT --to-destination "$peer_lan_ip"
      nexec "$ns" iptables -A FORWARD -i "$ext_if" -o "$lan_if" -j ACCEPT
      ;;
    restricted_cone) # 静态 DNAT + 仅放行"曾被联系过的源 IP"(不限端口)
      nexec "$ns" iptables -t nat -A PREROUTING -i "$ext_if" -j DNAT --to-destination "$peer_lan_ip"
      nexec "$ns" iptables -t mangle -A FORWARD -i "$lan_if" -o "$ext_if" -m recent --set --rdest --name contacted
      nexec "$ns" iptables -A FORWARD -i "$ext_if" -o "$lan_if" -m recent --rcheck --name contacted -j ACCEPT
      nexec "$ns" iptables -A FORWARD -i "$ext_if" -o "$lan_if" -j DROP
      ;;
    port_restricted) # 静态 DNAT + conntrack 精确 (ip,port) 放行
      nexec "$ns" iptables -t nat -A PREROUTING -i "$ext_if" -j DNAT --to-destination "$peer_lan_ip"
      nexec "$ns" iptables -A FORWARD -i "$ext_if" -o "$lan_if" -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
      nexec "$ns" iptables -A FORWARD -i "$ext_if" -o "$lan_if" -j DROP
      ;;
    symmetric) : ;; # 仅动态 SNAT(上方 --random-fully,按目的各自映射),无静态 DNAT——不加规则
  esac
}

# ---------- 测试网 TLS ----------
# 信令要求 TLS;二进制 --dev-gen-tls 生成的服务端证书只有 localhost/127.0.0.1
# SAN,跨 netns 以 203.0.113.1 访问会主机名校验失败,故 harness 自签含
# IP SAN 的证书。<dir> 由调用方给(一般放 case 临时目录)。
nm_gen_tls() { # <dir>
  local dir="$1"
  mkdir -p "$dir"
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$dir/ca.key" -out "$dir/ca.crt" \
    -days 2 -subj "/CN=MTDSS netns test CA" >/dev/null 2>&1 || return 1
  openssl req -newkey rsa:2048 -nodes -keyout "$dir/server.key" -out "$dir/server.csr" \
    -subj "/CN=$NM_PUB_IP_A" >/dev/null 2>&1 || return 1
  printf 'subjectAltName=IP:%s,DNS:localhost\n' "$NM_PUB_IP_A" >"$dir/san.cnf"
  openssl x509 -req -in "$dir/server.csr" -CA "$dir/ca.crt" -CAkey "$dir/ca.key" \
    -CAcreateserial -out "$dir/server.crt" -days 2 \
    -extfile "$dir/san.cnf" >/dev/null 2>&1 || return 1
  [ -s "$dir/ca.crt" ] && [ -s "$dir/server.crt" ] && [ -s "$dir/server.key" ]
}

# ---------- 服务进程(run_case 拉起/收尾) ----------
NM_PIDS=()

nm_start_servers() { # <case_dir>(日志与信令 db 落这里)
  local dir="$1"
  # STUN 绑定到通告 IP(而非 0.0.0.0):0.0.0.0 下回应的源地址取自出口
  # 网卡——回 gwB 一侧经 pub-b 出,源变 203.0.113.5,与通告的 203.0.113.1
  # 不符,客户端(conntrack 回程、connected socket、from!=server 校验)全会
  # 丢弃。绑 203.0.113.1 后回应源恒为通告地址,双腿一致。
  nexec "$NM_NS_PUB" env RUST_LOG="${NM_RUST_LOG:-info}" \
    "$STUN_BIN" --bind "$NM_PUB_IP_A:$NM_STUN_PORT" --bind-alt "$NM_PUB_IP_A:$NM_STUN_ALT" \
    >"$dir/stun.log" 2>&1 &
  NM_PIDS+=("$!")
  # 信令监听 0.0.0.0:8787 固定于二进制;db 为位置参数
  nexec "$NM_NS_PUB" env RUST_LOG="${NM_RUST_LOG:-info}" \
    "$SIGNALING_BIN" "$dir/signaling.db" \
    --tls-cert "$dir/tls/server.crt" --tls-key "$dir/tls/server.key" \
    --stun-addr "$NM_PUB_IP_A" --stun-port "$NM_STUN_PORT,$NM_STUN_ALT" \
    >"$dir/signaling.log" 2>&1 &
  NM_PIDS+=("$!")
  # 认证收口(v2-P1a):登录仅限预置账号,每个用例的全新 db 都要先经 admin
  # CLI 预置 dev@test,nm_register 的 login 才能过 request-code 的 404 门。
  # mtdss-admin 直写 SQLite:按 CREATE TABLE IF NOT EXISTS 幂等建表,与服务
  # 进程并存安全(WAL + rusqlite 默认 5s busy-timeout);-p signaling 构建
  # 同时产出该二进制。全仓仅此一处拉信令,run_case/chaos 全部用例覆盖。
  "$MTDSS_ADMIN_BIN" --db "$dir/signaling.db" user add dev@test >/dev/null ||
    nm_die "预置 dev@test 失败(mtdss-admin --db $dir/signaling.db user add)"
}

nm_wait_http_ready() { # [tries] 默认 40×0.5s=20s
  local tries="${1:-40}" i
  for i in $(seq 1 "$tries"); do
    # -k 轮询专用;任何 HTTP 应答(含 401)都算就绪
    if nexec "$NM_NS_PUB" curl -sk -o /dev/null --max-time 2 \
      "https://127.0.0.1:$NM_SIG_PORT/devices" 2>/dev/null; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

nm_stop_servers() {
  local p
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do
    kill "$p" 2>/dev/null || true
  done
  for p in ${NM_PIDS[@]+"${NM_PIDS[@]}"}; do
    wait "$p" 2>/dev/null || true
  done
  NM_PIDS=()
}

# ---------- 守护进程(run_case/chaos 拉起;常驻接收即收端) ----------
# 在 <ns> 里后台拉起 mtdssd:--recv-dir 显式(旧「b 侧 recv 后台进程」已删,
# daemon 常驻即接收器)、--log-file 落 JSON 行日志(断言面:srflx 候选行、
# 「接收完成」终局行、send attempt 失败行)、--no-tray(Linux 无托盘编译,
# 旗标恒被接受)。额外参数为 KEY=VALUE,直透进 daemon 进程环境(MTDSS_TEST_*
# 注入面移到 daemon 启动:编排选项在 daemon 内一次性读取)。
# stdio 与 --log-file 必须分离(落 ${log}.stdio):JSON logger 以追加模式
# 写 --log-file,shell 重定向 fd 却停在 offset 0——两 fd 同指一文件时,任何
# tracing 就绪前的 stderr 写入或 panic(默认 hook 走 stderr)会覆盖 JSON 日志
# 开头,毁掉失败路径的 dump 诊断件。
# 最近一次拉起的 pid 记在 NM_LAST_PID(run_case 的 auto-spawn 门禁要定点收)。
NM_LAST_PID=""
nm_start_daemon() { # <ns> <home> <log> <recv_dir> [KEY=VALUE...]
  local ns="$1" home="$2" log="$3" recv_dir="$4"
  shift 4
  mkdir -p "$home" "$recv_dir"
  nexec "$ns" env MTDSS_HOME="$home" RUST_LOG="${NM_RUST_LOG:-info}" "$@" \
    "$MTDSSD_BIN" --recv-dir "$recv_dir" --log-file "$log" --no-tray \
    >"$log.stdio" 2>&1 &
  local pid=$!
  NM_PIDS+=("$pid")
  NM_LAST_PID="$pid"
}

# 按 netns 全量收进程(auto-spawn 模式专用:CLI 自动拉起的 daemon 不在
# NM_PIDS;此刻 ns 里只可能剩它,防孤儿跨用例泄漏)
nm_kill_ns_procs() { # <ns>
  ip netns pids "$1" 2>/dev/null | xargs -r kill 2>/dev/null || true
}

# ---------- 设备登录(dev 模式验证码回环;走 mtdss login) ----------
# login 经 IPC 打到本侧 daemon:request_code RPC 把 dev 码回显到 stdout
# ("dev code: <code>"),再从 stdin 读一行回填 verify RPC。主进程持 FIFO
# 写端(fd9)避免 open 死锁,从 stdout 抓到码后写回 stdin——与旧 register
# 同型(抓码-回填),命令面换成 login。要求本侧 daemon 已起(先 nm_start_daemon)。
nm_register() { # <ns> <home> <email> <name>
  local ns="$1" home="$2" email="$3" name="$4"
  mkdir -p "$home"
  local fifo="$home/login.stdin" out="$home/login.out" err="$home/login.err"
  rm -f "$fifo"
  mkfifo "$fifo"
  timeout "${NM_REGISTER_TIMEOUT_S:-60}" ip netns exec "$ns" env \
    MTDSS_HOME="$home" RUST_LOG="${NM_RUST_LOG:-info}" \
    "$MTDSS_BIN" login --server "$NM_SERVER_URL" --email "$email" \
    --ca "$NM_CA_PEM" --name "$name" \
    <"$fifo" >"$out" 2>"$err" &
  local pid=$!
  exec 9>"$fifo" # 先开写端:子进程 stdin 的 open 解除后才会走到 request-code
  local code="" i
  for i in $(seq 1 60); do
    code="$(sed -n 's/^dev code: //p' "$out" 2>/dev/null | tail -n 1)"
    [ -n "$code" ] && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  if [ -z "$code" ]; then
    exec 9>&-
    wait "$pid" 2>/dev/null || true
    nm_log "login: 没等到 $email 的 dev 验证码"
    [ -f "$err" ] && sed 's/^/  [login] /' "$err" >&2
    [ -f "$out" ] && sed 's/^/  [login-out] /' "$out" >&2
    return 1
  fi
  printf '%s\n' "$code" >&9
  exec 9>&-
  if ! wait "$pid"; then
    nm_log "login: $email 非零退出"
    [ -f "$err" ] && sed 's/^/  [login] /' "$err" >&2
    return 1
  fi
  [ -s "$home/session.json" ] || {
    nm_log "login: $email 未产出 session.json"
    return 1
  }
}

# ---------- session.json / presence ----------
# 提取 session.json(serde 美化输出,逐行匹配足够)里的字符串字段
nm_json_str() { # <file> <key>
  sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" 2>/dev/null | head -n 1
}

# 键序日志(plan §4.2:互拨模型预期与两侧设备 pubkey 键序无关;把两侧
# pubkey hex 与字典序较小方打进日志,供 CI 归因与双键序回归核对)。
# 两侧各用自己的 JWT 拉 GET /devices,从中取两台设备的 pubkey。纯日志、
# 不门控:任何一步拿不到就提示跳过,绝不让用例失败。
nm_log_key_order() { # <dev_a> <home_a> <dev_b> <home_b>
  local dev_a="$1" home_a="$2" dev_b="$3" home_b="$4"
  local ja jb
  ja="$(nm_json_str "$home_a/session.json" jwt)"
  jb="$(nm_json_str "$home_b/session.json" jwt)"
  if [ -z "$ja" ] || [ -z "$jb" ]; then
    nm_log "key-order: 缺 JWT,跳过"
    return 0
  fi
  local side ns jwt resp line
  for side in a b; do
    if [ "$side" = a ]; then ns="$NM_NS_A"; jwt="$ja"; else ns="$NM_NS_B"; jwt="$jb"; fi
    resp="$(nexec "$ns" curl -sk --max-time 3 -H "Authorization: Bearer $jwt" \
      "$NM_SERVER_URL/devices" 2>/dev/null || true)"
    if [ -z "$resp" ]; then
      nm_log "key-order($side-JWT): /devices 无响应"
      continue
    fi
    line="$(printf '%s' "$resp" | python3 -c '
import json, sys
dev_a, dev_b = sys.argv[1], sys.argv[2]
try:
    data = json.load(sys.stdin)
except Exception:
    print("parse-failed")
    sys.exit(0)
pub = {}
for d in data.get("devices", []):
    pub[d.get("device_id")] = d.get("pubkey") or ""
pa, pb = pub.get(dev_a, ""), pub.get(dev_b, "")
if not pa or not pb:
    print("missing-pubkey")
elif pa == pb:
    print("tie")
else:
    smaller = "a" if pa < pb else "b"
    print("smaller=%s a_pub=%s b_pub=%s" % (smaller, pa, pb))
' "$dev_a" "$dev_b" 2>/dev/null || echo "python-failed")"
    nm_log "key-order($side-JWT): $line"
  done
}

# presence 就绪:轮询 a 侧 `mtdss devices`(经 IPC 读 daemon 在线循环维护
# 的目录快照)直到 <device_id> 行出现且 online=true。旧实现以 a 侧 JWT 直拉
# REST;换轨后快照权威在 daemon,顺带覆盖「在线循环确实建连」的路径。
nm_wait_online() { # <device_id> <home_a> [tries=40]
  local want="$1" home="$2" tries="${3:-40}" i out
  for i in $(seq 1 "$tries"); do
    out="$(nexec "$NM_NS_A" env MTDSS_HOME="$home" "$MTDSS_BIN" devices 2>/dev/null || true)"
    if printf '%s\n' "$out" | grep -q "^${want}"$'\t'".*online=true"; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

# 收侧 sha256 清单快照(与 payload.sh 输出同序;落盘断言轮询共用)
nm_manifest_snapshot() { # <out_dir> <dst_file>
  (
    cd "$1" &&
      find . -maxdepth 1 -type f -name '*.bin' -printf '%f\n' |
      LC_ALL=C sort | xargs -r sha256sum
  ) | LC_ALL=C sort >"$2"
}

# 日志终局行有界轮询:传输成功、清单落盘齐之后,接收方的 EOF 收尾舞步
# (≤2s)可能晚于清单轮询通过那一刻才把终局行写进日志——一次性 grep 属
# TOCTOU,偶发 CI 红。终局行晚于文件落盘属正常时序,必须有界轮询而非
# 立判失败。
# 用法: nm_wait_log <file> <pattern> <timeout_s>;每 0.5s grep -q 一次,
# 命中返回 0,超时返回 1(与 nm_wait_http_ready / nm_wait_online 同型)。
nm_wait_log() { # <file> <pattern> <timeout_s>
  local file="$1" pattern="$2" timeout_s="$3" i
  local tries=$((timeout_s * 2))
  for i in $(seq 1 "$tries"); do
    if grep -q "$pattern" "$file" 2>/dev/null; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}
