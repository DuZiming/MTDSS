# CI flake 治本报告:kill-resume 发布集合断言族(PR #22 连续两跑同钉)

**日期**:2026-11-03 · **分支**:worktree-worktree-mtdss-v2p5-ios · **基线**:fa085ba
**证据**:GitHub Actions ubuntu,`cargo test --workspace`(全量并行),`anonymous_resend_after_kill_resumes_and_publishes` 连续两跑在同一处 panic:

```
crates/daemon/tests/inbox_layout_e2e.rs:678:13
assertion left == right failed: 恰一次发布: []
left: 0, right: 1     (套件 ~46s 处)
```

R7 修复轮账上该族标注「全量并行抖动→独占重跑全绿」;CI 现为**确定性红**(同钉连两跑),rerun-roulette 判死,升级治本。

## 1. 诊断:发布集合断言以 A 侧网络面为门,与 B 侧发布链零顺序

失败断言读的是伪 App 的 `fs.publish` 请求登记簿(`log2.seen`),但门是 **A 侧** `transfer.wait` 的终态 done——只钉「A 把字节推完」,对 B 侧发布链**没有任何顺序承诺**。

生产序链(jobs.rs / inbox_dest.rs,逐环钉死):

1. B 收轮落定 → `begin_publish`(jobs.rs:2521)→ `mark_round_publishing`(卡翻 publishing);
2. `publish_coordinator`(jobs.rs:3208)逐文件发 `fs.publish`、等 0x05;
3. 伪 App `handle_event` 在**事件到达时**先登记 `seen`(inbox_layout_e2e.rs:245),然后 move 暂存件、回 0x05 ok;
4. 协调器收齐 0x05 → `settle_published` 删暂存件 + 记录打 `published:true`(inbox_dest.rs:221,顺序钉死注释)→ `settle_publish(true)`(jobs.rs:3256);
5. download 驱动的 done 合成门**要求发布相位 Settled** 才落 done(jobs.rs:3974-3984;匿名卡由协调器直接 `finish("done")`,jobs.rs:3257-3258)。

推论:

- **A 侧 done ⇒ 无任何 B 侧承诺**(步骤 1-5 全部未序)。全量并行慢机上「A done → B 派发 fs.publish → App 事件循环唤醒 → seen 登记」的窗可任意长(CI 两跑都落在窗内,seen 为空)。
- **记录 done+published ⇒ 步骤 4 完成 ⇒ 步骤 3 已发生 ⇒ seen 必已登记**(seen 登记早于 0x05 发送,记录打标晚于 0x05 收讫——两头夹死)。
- **B 侧卡 done ⇒ settle_publish(true) 已落 ⇒ 同上**。

病灶即:原断言把 `seen.len()==1` 放在 `wait_record(done+published)` **之前**,而后者就写在紧邻下一行——门信号早已存在,只是排错了序。

## 2. 修复(纯测试侧,零产品码改动)

### inbox_layout_e2e.rs `anonymous_resend_after_kill_resumes_and_publishes`(CI 红钉)

发布集合断言移到 `wait_record(done+published)`(既有 90s 轮询门)之后。语义不变(恰一次、发布名=rel),门换成生产序保证面;注释写明 A/B 两侧序窗依据与 CI 证据。轮询门而非 sleep。

### inbox_staging_e2e.rs ② `inbox_publish_timeout_fails_and_retains_staging`(族内唯一残余非严格序门)

`seen.len()==1` 原以 B 侧 `failed` 为单查门。failed 由协调器**发布超时(注入 1500ms)**产生,与伪 App 事件循环登记 seen 之间无生产序——慢机 App 滞后超注入超时窗即同型红(理论窗,无 CI 实证)。加固:轮询至恰一(30s 预算,10ms 步进,与文件内既有 T2-③ hold 窗轮询同款),预算尽仍空才红(红时消息带 seen 现场)。`answered` 全空断言(Ignore 模式恒真面)保持原位。

## 3. 家族清扫:两文件全部 13 处发布/查询登记断言面逐一验序

门=**B 侧发布收口信号**(记录 done+published / 卡 done / 卡 failed 含应答计数)者,按 §1 推论均为严格序,不动:

| 文件:行(修后) | 用例 | 门 |
|---|---|---|
| layout:686→691 | anonymous_resend(本钉) | **wait_record done+published(新挪)** |
| layout:759→763 | layout_full_chain 首发面 | wait_incoming done |
| layout:795→798 | layout_full_chain Skip 重发 | wait_incoming card2 done |
| layout:902→907 | kill_mid_receive | wait_card(cb2) done |
| staging:626→633 | ① 全链 | wait_incoming done |
| staging:715→717 | ② 发布超时 | **轮询恰一(新加固)** |
| staging:812 | ③ 误 Skip 守卫 | wait_incoming card2 done |
| staging:831→837 | ③ 终局 final_rel | wait_record done+published |
| staging:900 | ⑤ 并发发布单败 | wait_incoming failed(失败计数=全部应答已收) |
| staging:988→995 | ⑥ Skip 锚 | wait_incoming card2 done |
| staging:1348→1353 | T2-① 分区续传 | wait_card done(先 publishing 后 done) |
| staging:1479 | T2-② republish-only | 逐 rel wait_record done+published |
| staging:1594-1596 | T2-③ hold 窗 | 既有轮询 |
| staging:1655 | T2-③ 收尾 | wait_card done |

`queries`(0x04)/`answered` 断言同门随行,均为严格序(Skip 判定前置依赖查询应答;failed 计数前置依赖全部应答收讫)。

**排除面(查过、非病灶)**:
- kill 窗字节账(1..=48 块窗、pause+300ms 排干、短推 <90% 上界):落盘批量 16 块/2s 先到者保证杀点 ≥16 块(25%)→ 短推上界 75%<90% 恒成立,窗检查是轮询非单查;
- 重启后 app2 订阅先于重发起跑(连接即订阅,广播通道留存),无丢事件窗;
- 广播 Lagged 跳事件属另一族(产品通道容量),两处 CI 证据均非此形态,不扩波。

## 4. 验证(WSL 副本 /root/mtdss-flake = fa085ba + 本波两文件,库复用 mtdss-ci 暖 target)

- **Windows 本地**:`cargo test -p mtdss-daemon --test inbox_layout_e2e --test inbox_staging_e2e` 编译绿、跑 0 例(整文件 `#![cfg(unix)]`,Windows 无 fdpass——语法面过,类型面在 WSL 验)。
- **WSL 两钉测 `--test-threads=4`**:`inbox_layout_e2e` 5/5(65.58s,含本钉 anonymous_resend);`inbox_staging_e2e` 9/9(58.02s,含加固的 ②)。
- **WSL `cargo test --workspace`(默认并行=6 线程,CI 同形)run 1**:全部目标绿,**唯 `update_state_stream_three_branches` 红**(crates/cli/tests/ipc_cli.rs:1101,`mtdss 运行超 90s…args=["update"]`)——此为控制者 PR #22 战况已账的另一族(CI 二跑同签名、rerun 绿;「CI 慢机预算放宽」候选票),**非本波家族、非本波改动面**(本波零产品码、不动 cli);本家族全部目标(run 1 含)全绿。
- **WSL `cargo test --workspace` run 2(背靠背负载模拟)**:**全绿**(69 目标 RC=0、零 FAILED)——`ipc_cli` 28/28(update_state_stream 本跑绿,1.17s)、`inbox_layout_e2e` 5/5(56.42s)、`inbox_staging_e2e` 9/9(53.33s)。本家族在满载续跑下两跑均无一红。
- **零产品码改动**:diff 仅两测试文件(31 增 7 删,含注释)。

## 5. 遗留

- **本波家族已收口**;PR #22 重触 CI 由控制者执行(本分支推送 macmini,origin 推送与 CI 重跑归控制者)。
- **update_state_stream 预算票新增本地实证**:WSL 全量并行 run 1 复现同签名(90s 子进程预算超时,90.71s 处红)。慢机预算放宽票(CI 慢机预算放宽+kill-resume 族时序加固之后半)已有 CI 1 + WSL 1 两笔实证,建议 merge 后按既有候选票处理。
- ② 的理论窗若实证化(广播丢事件/App 滞后 >1.5s 实测),再开产品侧票;本轮不扩。
