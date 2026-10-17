#!/usr/bin/env bash
# lib.sh — netns NAT 矩阵公共库:编址常量、二进制发现、NAT 规则配方、
# 测试网 TLS、设备注册(dev 验证码回环)、presence 轮询等。
# 被 setup.sh / teardown.sh / run_case.sh source。
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
if [ -n "${MTDSS_BIN_DIR:-}" ]; then
  NM_BIN_DIR="$MTDSS_BIN_DIR"
else
  # 脚本目录的上两级 = 仓库根
  NM_REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
  NM_BIN_DIR="$NM_REPO_ROOT/target/release"
fi
MTDSS_BIN="$NM_BIN_DIR/mtdss"
SIGNALING_BIN="$NM_BIN_DIR/signaling-server"
STUN_BIN="$NM_BIN_DIR/stun-server"

# ---------- 小工具 ----------
nm_log() { printf '[nat-matrix] %s\n' "$*"; }
nm_die() { nm_log "ERROR: $*" >&2; exit 1; }

# 运行前置:root + 关键命令 + 三个 release 二进制
nm_require_env() {
  [ "$(id -u)" -eq 0 ] || nm_die "需要 root(CI 用 sudo 跑;本机 Linux 复跑: sudo bash scripts/nat-matrix/matrix.sh)"
  local c
  for c in ip iptables sysctl timeout dd sha256sum openssl curl mkfifo seq sort xargs python3; do
    command -v "$c" >/dev/null 2>&1 || nm_die "缺命令: $c"
  done
  [ -x "$MTDSS_BIN" ] || nm_die "找不到 $MTDSS_BIN(先: cargo build --release -p mtdss-cli -p signaling -p stun-server)"
  [ -x "$SIGNALING_BIN" ] || nm_die "找不到 $SIGNALING_BIN"
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
  # 公共:固定外地址、保端口 SNAT + 转发放行出向
  nexec "$ns" iptables -t nat -A POSTROUTING -o "$ext_if" -j SNAT --to-source "$ext_ip"
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
    symmetric) : ;; # 仅动态 SNAT(按目的各自映射),无静态 DNAT——公共两条之外不加规则
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

# ---------- 设备注册(dev 模式验证码回环) ----------
# register 先 POST /auth/request-code 并把 dev 验证码打到 stderr
# ("dev-mode code for <email>: <code>"),再从 stdin 读一行。主进程持 FIFO
# 写端(fd9)避免 open 死锁,从 stderr 抓到码后写回 stdin——全程真实 CLI 路径。
nm_register() { # <ns> <home> <email> <name>
  local ns="$1" home="$2" email="$3" name="$4"
  mkdir -p "$home"
  local fifo="$home/reg.stdin" err="$home/register.err"
  rm -f "$fifo"
  mkfifo "$fifo"
  timeout "${NM_REGISTER_TIMEOUT_S:-60}" ip netns exec "$ns" env \
    MTDSS_HOME="$home" RUST_LOG="${NM_RUST_LOG:-info}" \
    "$MTDSS_BIN" register --server "$NM_SERVER_URL" --email "$email" \
    --name "$name" --ca "$NM_CA_PEM" \
    <"$fifo" >"$home/register.out" 2>"$err" &
  local pid=$!
  exec 9>"$fifo" # 先开写端:子进程 stdin 的 open 解除后才会走到 request-code
  local code="" i
  for i in $(seq 1 60); do
    code="$(sed -n 's/^dev-mode code for .*: //p' "$err" 2>/dev/null | tail -n 1)"
    [ -n "$code" ] && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.5
  done
  if [ -z "$code" ]; then
    exec 9>&-
    wait "$pid" 2>/dev/null || true
    nm_log "register: 没等到 $email 的 dev 验证码"
    [ -f "$err" ] && sed 's/^/  [register] /' "$err" >&2
    return 1
  fi
  printf '%s\n' "$code" >&9
  exec 9>&-
  if ! wait "$pid"; then
    nm_log "register: $email 非零退出"
    [ -f "$err" ] && sed 's/^/  [register] /' "$err" >&2
    return 1
  fi
  [ -s "$home/session.json" ] || {
    nm_log "register: $email 未产出 session.json"
    return 1
  }
}

# ---------- session.json / presence ----------
# 提取 session.json(serde 美化输出,逐行匹配足够)里的字符串字段
nm_json_str() { # <file> <key>
  sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" 2>/dev/null | head -n 1
}

# 轮询 /devices 直到 <device_id> 在线(用 <jwt_home> 的 Bearer 令牌)。
# 小账号只有收发两台设备,以"目标 id 在目录 且 响应含 online:true"判就绪。
nm_wait_online() { # <device_id> <jwt_home> [tries=40]
  local want="$1" jwt_home="$2" tries="${3:-40}" i jwt resp
  jwt="$(nm_json_str "$jwt_home/session.json" jwt)"
  [ -n "$jwt" ] || return 1
  for i in $(seq 1 "$tries"); do
    resp="$(nexec "$NM_NS_A" curl -sk --max-time 2 \
      -H "Authorization: Bearer $jwt" "$NM_SERVER_URL/devices" 2>/dev/null || true)"
    if printf '%s' "$resp" | grep -q "\"device_id\"[[:space:]]*:[[:space:]]*\"$want\"" &&
      printf '%s' "$resp" | grep -q '"online"[[:space:]]*:[[:space:]]*true'; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}
