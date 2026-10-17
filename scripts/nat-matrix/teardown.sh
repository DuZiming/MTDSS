#!/usr/bin/env bash
# teardown.sh — 幂等清场:gw 侧 nat/mangle 逐表刷掉(plan 定稿口径),
# 再删全部五个 netns。缺什么跳什么,重复执行安全。恒 exit 0。
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$HERE/lib.sh"

# 网关 ns 还在时先把规则刷干净(逐表),再删 ns
for ns in "$NM_NS_GWA" "$NM_NS_GWB"; do
  if nm_ns_exists "$ns"; then
    ip netns exec "$ns" iptables -t nat -F 2>/dev/null || true
    ip netns exec "$ns" iptables -t mangle -F 2>/dev/null || true
    ip netns exec "$ns" iptables -t filter -F 2>/dev/null || true
  fi
done

for ns in "$NM_NS_A" "$NM_NS_B" "$NM_NS_GWA" "$NM_NS_GWB" "$NM_NS_PUB"; do
  ip netns del "$ns" 2>/dev/null || true
done

exit 0
