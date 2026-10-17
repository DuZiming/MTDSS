# MTDSS 发布检查单

## 跨真实网络对传（V1 发布前执行一次，记录归档本目录）

1. 两处不同 NAT 网络（如 家宽 × 手机热点）各一台设备，各自 `mtdss register`
2. `RUST_LOG=info` 下多文件对传，记录两侧日志的 `candidates:` 行（srflx 值）与 `symmetric=` 预判
3. 记录：日期 / 两侧网络环境 / srflx 是否出现 / 传输结果（成功字节数或失败文案）

- [ ] 家宽 × 热点 对传记录：____

## STUN / 部署

- 自建 STUN 双腿部署须显式绑定对外通告 IP（`--bind <advertised-ip>:port`），
  `0.0.0.0` 绑定会让回包源地址随出口腿漂移（Task 8 根因：客户端按回复源
  地址比对候选，漂移即整条腿作废）。

## 已知问题（V1 发布前处理）

- `.mtdss` 保留段匹配为大小写敏感——Windows 大小写不敏感卷上 `.MTDSS`
  载荷可落进隐藏收件区；V1 发布前硬化为大小写不敏感拒绝
  （`crates/cli/src/flow.rs` `walk_dir` 与 `crates/core/src/protocol/manifest.rs`
  `validate_rel_path` 两处）。
