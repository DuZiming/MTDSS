#!/usr/bin/env bash
# setup.sh — 建五个 netns + veth 编址 + 网关转发开关 + 双侧 NAT 类型规则。
# 幂等:开头先跑 teardown 清场。不落任何常驻进程(信令/STUN 由 run_case 拉起)。
# 两侧 NAT 类型经环境传入:NM_TYPE_A / NM_TYPE_B(缺省 full_cone)。
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

TYPE_A="$(nm_norm_type "${NM_TYPE_A:-full_cone}")"
TYPE_B="$(nm_norm_type "${NM_TYPE_B:-full_cone}")"

bash "$HERE/teardown.sh"

# veth 成对创建并挪进各 ns;命名 <ns>-ext/<ns>-lan 可预测,pub 侧叫 pub-a/pub-b
mk_pair() { # <left_link> <left_ns> <right_link> <right_ns>
  ip link add "$1" type veth peer name "$3"
  ip link set "$1" netns "$2"
  ip link set "$3" netns "$4"
}

for ns in "$NM_NS_PUB" "$NM_NS_GWA" "$NM_NS_A" "$NM_NS_GWB" "$NM_NS_B"; do
  ip netns add "$ns"
  ip -n "$ns" link set lo up
done

mk_pair pub-a "$NM_NS_PUB" gwA-ext "$NM_NS_GWA" # 链 A:203.0.113.0/30
mk_pair gwA-lan "$NM_NS_GWA" a-lan "$NM_NS_A"   # lan A:10.0.1.0/30
mk_pair pub-b "$NM_NS_PUB" gwB-ext "$NM_NS_GWB" # 链 B:203.0.113.4/30
mk_pair gwB-lan "$NM_NS_GWB" b-lan "$NM_NS_B"   # lan B:10.0.2.0/30

# 编址(plan 定稿)
ip -n "$NM_NS_PUB" addr add "$NM_PUB_IP_A/30" dev pub-a
ip -n "$NM_NS_PUB" addr add "$NM_PUB_IP_B/30" dev pub-b
ip -n "$NM_NS_GWA" addr add "$NM_GWA_EXT_IP/30" dev gwA-ext
ip -n "$NM_NS_GWA" addr add "$NM_A_LAN_GW/30" dev gwA-lan
ip -n "$NM_NS_GWB" addr add "$NM_GWB_EXT_IP/30" dev gwB-ext
ip -n "$NM_NS_GWB" addr add "$NM_B_LAN_GW/30" dev gwB-lan
ip -n "$NM_NS_A" addr add "$NM_A_LAN_IP/30" dev a-lan
ip -n "$NM_NS_B" addr add "$NM_B_LAN_IP/30" dev b-lan

ip -n "$NM_NS_PUB" link set pub-a up
ip -n "$NM_NS_PUB" link set pub-b up
ip -n "$NM_NS_GWA" link set gwA-ext up
ip -n "$NM_NS_GWA" link set gwA-lan up
ip -n "$NM_NS_GWB" link set gwB-ext up
ip -n "$NM_NS_GWB" link set gwB-lan up
ip -n "$NM_NS_A" link set a-lan up
ip -n "$NM_NS_B" link set b-lan up

# 路由:设备默认走网关,网关默认走 pub。gwB 的下一跳 203.0.113.5 本就在
# gwB-ext 的连接网段(203.0.113.4/30)内,onlink 为冗余但无害的显式标注。
ip -n "$NM_NS_A" route add default via "$NM_A_LAN_GW"
ip -n "$NM_NS_B" route add default via "$NM_B_LAN_GW"
ip -n "$NM_NS_GWA" route add default via "$NM_PUB_IP_A"
ip -n "$NM_NS_GWB" route add default via "$NM_PUB_IP_B" dev gwB-ext onlink

# 网关转发开关(两侧)
nexec "$NM_NS_GWA" sysctl -qw net.ipv4.ip_forward=1
nexec "$NM_NS_GWB" sysctl -qw net.ipv4.ip_forward=1

# 双侧 NAT 类型规则(同一配方)
nm_apply_nat_rules "$NM_NS_GWA" gwA-ext gwA-lan "$NM_GWA_EXT_IP" "$NM_A_LAN_IP" "$TYPE_A"
nm_apply_nat_rules "$NM_NS_GWB" gwB-ext gwB-lan "$NM_GWB_EXT_IP" "$NM_B_LAN_IP" "$TYPE_B"

nm_log "setup ok: gwA=$TYPE_A gwB=$TYPE_B"
