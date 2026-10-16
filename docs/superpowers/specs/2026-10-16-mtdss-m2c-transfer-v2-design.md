# MTDSS M2c 设计文档：传输 v2（多文件/目录、断点续传、并发调度）

- 日期：2026-10-16
- 状态：待评审（brainstorm 对话定稿）
- 上游规范：[2026-10-15-mtdss-v1-design.md](2026-10-15-mtdss-v1-design.md) §6、§10、§11
- 子项目定位：M2a（债务清偿，已合并 d97a46e）→ M2b（NAT 打洞，已合并 91dda77）→ **M2c（传输 v2）** → M2d（NAT 矩阵 + 混沌验收）

## 1. 目的与范围

M1/M2b 的传输是单文件、顺序分块、断了即败。M2c 交付"传输 v2"：

| 交付 | 内容 |
|---|---|
| 多文件/目录 manifest | `verify` 放开单文件限制；目录结构经 rel_path 还原；CLI 多路径发送 |
| 隐藏收件区 | `out_dir/.mtdss/`：`.part` 分片临时文件 + 对账记录 + 永久完成索引 |
| 接收计划协议 | manifest 之后接收端逐文件回 跳过/全量/续传+bitmap，发送端只推计划内块 |
| 并发调度 | 先小后大，4 文件并发，每文件 4 条在途块流 |
| 会话内断线重连 | 指数退避 5 次，重走信令握手 + 互拨，复用同一 manifest |
| 死链检测（保活） | 双角色端点统一注入 QUIC keep-alive + 空闲超时（v1 §5.5 遗留承诺在此落地），静默断链（对端进程消失/NAT 重绑）可检测、重连机制可触发 |
| 失败报告与手动重发 | 重连耗尽按用户原始路径报告成败清单；手动重发自动跳过/续传/新传 |
| M2b 债务清偿 | ClientEvent 语义统一（随重连）+ 五笔小债（§9） |

**范围外**：暂停/取消显式协议（M3 daemon——CLI 里 Ctrl-C 直接退出，`.part` 与记录留作手动重发的续传素材）；多任务并行队列（M3）；跨会话**自动**恢复（本里程碑只做手动重发即续传，不自动发起）；netns NAT 矩阵与 netem 混沌（M2d）；加密中继（V2）。

## 2. 核心裁定（brainstorm 定稿）

1. **对账驱动一切**：接收端磁盘状态是唯一事实源。初次发送、断线重连、手动重发在协议上完全同构——发送端发起握手 → 发 manifest → 接收端对账回"接收计划" → 发送端只推计划内块。接收端没有也不需要"重连"概念。
2. **隐藏收件区统一三种场景**：单文件、多文件、目录完全同一套；目标目录里只见"已完成的正式文件"与隐藏区，不存在散落的中间态文件。
3. **完成后只清临时文件**：`.part` 随各自文件完成即删；完成记录永久保留，作为"已收索引"支撑后续手动重发的跳过判定。
4. **文件身份 = (rel_path, size, 整文件 sha256)**，与 transfer_id 无关——手动重发是新 transfer_id，对账必须跨 attempt 成立。
5. **续传语义分层**：会话内断线自动重连续传（用户无感）；多次重试耗尽即判定中断、报告；恢复只能由用户手动重新发起（自动跳过已完成、续传半程、新传未开始）。

## 3. 隐藏收件区（`core` 新模块 `intake`）

```
<out_dir>/.mtdss/
  <sha256hex(rel_path)>.part     # 预分配分片临时文件（未完成时存在）
  <sha256hex(rel_path)>.json     # 记录：{ rel, size, sha256, chunk_count, bitmap(hex), done, final_rel }
```

- 键 = sha256(rel_path) 全长 hex：规避路径字符集、长度与平台保留名（CON、尾点等）问题；身份不符由记录内容自判（哈希对不上 → 作废重传）。
- bitmap：每块 1 bit、`ceil(n/8)` 字节，hex 存记录；记录按批落盘（每 16 块或 2s 或 FileDone 时重写）——崩溃代价 ≤16 块重传（幂等，接收端按哈希验后覆写），换取大文件下写放大有界（64 GB 文件 bitmap 约 8 KB，逐块全量重写会到 0.8% 且线性增长）。
- 完成次序（**先改名后记 done**）：全块齐 → 整文件哈希验证 → `unique_target` 占位 → `.part` 原子改名覆盖占位（沿用 v1 冲突改名 `name (1).ext` 语义）→ 记录落 `done=true` + `final_rel`（实际落盘相对名，含冲突改名后缀）→ 删 `.part` 留记录。
  - rename 失败（如 Windows 保留名 CON）：记录保持 partial + 全 1 bitmap，干净报错；后续手动重发走 RESUME(缺块=∅) → 仅复核 + 重试改名，**不重传文件数据**。保留名最终失败为文档化限制（§10），V1 不做名字转写。
  - 崩溃窗口（已改名未记 done）：`.part` 已不在 → 对账按 FULL 重传一次该文件，可接受。
- 对账规则（manifest 条目 vs 本地状态）：

| 本地状态 | 判定 |
|---|---|
| `done &&` `final_rel` 存在且 size 吻合 | SKIP |
| 记录哈希吻合 `&&` 有 `.part` | RESUME（缺块 = bitmap 反相） |
| 其余（无记录 / 哈希不符 / 缺 `.part`） | FULL（作废并删除旧记录/旧 `.part`） |

- 陈旧 `.part` 由对账作废；`recv` 启动时的 `sweep_part_files` 盲扫**移除**（会误删续传素材，正是本次要修的）。
- Windows 上 `.mtdss` 目录设隐藏属性（`std::os::windows::fs`）；其他平台点前缀即约定。
- 接收目录被两个 recv 进程并发使用不在本里程碑范围（CLI 单传输；M3 daemon 再论）。

## 4. 传输协议 v2（QUIC 流之上）

### 4.1 控制流时序

```
S: open_bi → [u32 BE man_len][manifest bytes]
R: 对账 → [u8 verdict=1][u32 BE file_count]
         每文件 [u8 kind][u32 BE bmp_len][bmp]      # kind: 0=FULL 1=SKIP 2=RESUME
                                                    # FULL/SKIP 无 bmp；RESUME 位=1 表示缺
   或 [u8 verdict=0][u32 BE reason_len][reason]      # 空间不足等，整体拒绝
S: 按 §5 调度推块 → 计划内全部推完 [0xFF checkpoint]
R: 每文件完成即时回 [u8 0x02][u32 BE file_idx]       # FileDone（发送端报告用）
R: checkpoint 复核：仍有缺块 → 再发一帧接收计划（S 继续，≤3 轮）
                    全齐（各文件整体验证+落盘完成）→ [0x01 final]
S: 收 0x01 → finish 控制流发送侧 → EOF 舞步同 v1
```

### 4.2 块流（uni）

`[u32 BE file_idx][u64 BE offset][u64 BE length][data]`——file_idx 为 manifest 下标；(file_idx, offset) 必须命中签名 manifest 的块表，否则拒收。length ≤ 1 MiB 校验同 v1。**流帧格式不做跨版本兼容**（发收双方同版本；manifest protobuf 本身零改动，老收端对新发端的多文件 manifest 在 `verify` 处干净报错，不会走到块流）。

### 4.3 manifest

- proto 零改动（repeated files 本就在）；`verify` 放开"恰好 1 个"→ ≥1，新增 rel_path 去重校验 + **首段保留名 `.mtdss` 拒绝**（防已认证对端把终名落进/覆盖隐藏收件区——这是信任边界不变式，哈希键替代不了它）；编码上限 64 MiB、文件数 ≤ 65535。
- transfer_id 保留（随机 hex32 校验不变），仅作日志关联，不再参与任何文件命名。

### 4.4 恶意与异常有界性

- checkpoint 重请求硬上限 3 轮，超出判失败（防恶意收端无限诱导重发、防发端无限缺块死循环）；
- 块哈希不符：不落盘、不进 bitmap，自然落入下一轮缺块集；
- 发送端源文件传输中被修改：块数据与 manifest 哈希不符 → 重请求轮耗尽 → 干净失败（文档化：传输期间勿改源文件）；
- 伪造块（file_idx 越界 / offset 不在块表 / 长度超限）：拒收该流；
- 恶意收端谎报"全 SKIP"只害其自身（发送端如实按 FileDone 汇报，与 v1 ack 信任模型一致）。

### 4.5 core API 形变

```rust
pub struct SendOptions { pub max_concurrent_files: usize, pub max_inflight_per_file: usize } // Default: 4 / 4
pub enum SendProgress<'a> { FileDone { rel: &'a str }, Bytes { rel: &'a str, done: u64, total: u64 } }
/// 终局/可重试分类：编排层只对 Network 重试（§6），Rejected/Protocol 立即终局进报告。
pub enum SendError { Network(String), Rejected { reason: String }, Protocol { reason: String } }

pub fn build_manifest(items: &[(PathBuf, String /*rel*/)], id: &DeviceIdentity) -> Result<Manifest>  // 一次性构建+签名
pub async fn send_files(conn, man: &Manifest, sources: &[(PathBuf, String)], progress, opts) -> Result<SendReport, SendError>
pub async fn recv_files(conn, out_dir, me) -> Result<Vec<PathBuf>>   // 签名不变，内部 v2
```

- **manifest 归编排层所有**：`build_manifest` 构建一次，重连循环持有并跨 attempt 传同一引用——不重哈希源文件（大文件重试场景正是续传存在的理由）、transfer_id 恒定、重发字节严格相同。手动重发（§7）才会重建。
- v1 的 `send_file` 删除（调用方统一 `send_files`，既有 e2e 测试随迁）。core 不持有跨连接状态：重试循环在编排层（§6）。

## 5. 调度（发送端）

- 文件按 size 升序入队（先小后大，尽快出现完成项）；并发文件流 `max_concurrent_files = 4`；
- 每文件块队列，同时在途 uni 流 `max_inflight_per_file = 4`（总在途 ≤16，quinn 默认 uni 并发上限 100，余量充足）；
- 背压：纯 QUIC 流控（v1 §6.6 不变），不自研限速；
- 两个常量经 `SendOptions` 注入，测试可缩到 1×1。

## 6. 断线重连（编排层，`cli/flow`）

**前提：死链可检测**。quinn 默认无 keep-alive、无空闲超时——对端静默死亡（进程消失/断电/NAT 重绑）时读写永久挂起，重连根本不会发生。M2c 在 `make_dual_endpoint`（及其 client 侧配置）统一注入 `TransportConfig`：`keep_alive_interval = 10s`、`max_idle_timeout = 30s`（经端点构造参数注入，测试用亚秒级）。静默断链 ≤30s 内以读写错误显形 → 进入下述循环；30s 远小于接收端窗口，慢而活的传输不受误伤（传输期 ACK 常流，静默即死）。

```
发送端：
for attempt in 0..=MAX_RECONNECTS(5):
    attempt > 0 → sleep(2^(attempt-1) s)          # 1,2,4,8,16
    （信令 WS 已死则先重建）
    ConnectRequest → Accept → 候选交换 → punch_connect   # 全同 M2b 握手；session_id 每次新
    send_files(同一 manifest 内存副本) →
        Ok 即成；SendError::Network → 重试
        Rejected/Protocol → 立即终局（重试不可能成功：磁盘不足/源文件被改/协议违规）
耗尽 → §7 失败报告

接收端：
连接/信令断 → 重回 wait_connect_request（窗口 RECV_RECONNECT_WINDOW = 300s）
窗口内来件（自动重连或手动重发，无从区分）→ 同一 对账→计划 流程
超窗 → 判失败退出
```

- **重连 = 发送端自动重发起完整握手**；接收端不需要区分三种来件来源（初次/重连/手动重发）。
- 发送端全程持有 manifest 内存副本，重连重发同一字节——对账靠文件身份，天然跨 attempt。
- 信令 WS 中途死亡等价于一次断线：双方各自重建信令连接再走上述流程（ClientEvent 语义统一见 §9 第一行）。
- 接收窗口默认 300s 的推导：覆盖发送端最坏单次 attempt（信令重建 + Accept + 候选交换各 10s + gather 4s + punch 10s ≈ 44s）×5 + 退避 31s ≈ 251s，留裕量。真实失败场景下接收端多滞留几分钟无害（本就是等待进程）。
- 重试预算、退避序列、空闲超时与接收窗口均可注入（测试用秒级）。

## 7. 失败报告与手动重发

- 发送端逐文件跟踪：收到 FileDone = 成；manifest 内其余 = 未成。
- 重连耗尽后 CLI 报告（非零退出码）：按用户**原始输入路径**列清单——已完成哪些、未完成哪些（能区分"传了一半"与"未开始"，由最后已知计划进度推得）；发起时记录 输入路径 → rel 前缀 映射用于还原措辞。
- 手动重发 = 再次对同一路径集合执行 send：发送端重建 manifest（文件未变则哈希不变），接收端对账自动跳过/续传/新传。全部 SKIP 时发送端零字节直收 final，报告"对端已全部持有"。

## 8. CLI

- `mtdss send <PATH...> --to <id>`：位置参数变多值；文件 → rel = 文件名；目录 → 递归收集，rel = `目录名/子路径`（scp -r 语义保结构）；
- 发起前预检：路径不存在即错；空目录警告跳过（全部为空则报错）；符号链接跳过 + 警告；不同输入产生相同 rel → 发起前报错拒绝；首段为 `.mtdss` 的输入（目录恰好叫这个名字）→ 发起前报错（对端 verify 也会拒，预检只是提前给用户好报错）；
- `recv --out` 不变；隐藏区位于 out 之下。

## 9. M2b 债务清偿（随本里程碑）

| 债 | 处置 |
|---|---|
| ClientEvent 语义统一 | 信令 WS 任务任何退出路径（sink 写错误/对端关闭/keepalive 判死）统一产生确定事件序列：可诊断 `Error`（含原因）→ `Closed`；调用方一律以 Closed 终止等待，重连编排（§6）负责重建 |
| 单端口 STUN 通告 | 客户端 `setup_punch_endpoint`：通告仅 1 条时 `alt = primary`（对称预判退化为恒 false，无害），不再落入"仅本地候选"降级 |
| `--stun-port` 校验 | 信令服务端拒绝 0 端口与重复端口 |
| RFC1918 断言恢复 | `stun_client.rs` 的 local_addrs 测试恢复私网优先排序断言（环境无私网地址时跳过该断言，而非弱化） |
| run_listener 错误续行 | `recv_from`/`send_to` 瞬时错误记日志继续，不再整体退出 listener |
| `--all-targets` clippy | `cargo clippy --workspace --all-targets -D warnings` 清零，并自本里程碑起纳入常态门禁 |

## 10. 错误与诊断

| 场景 | 行为 |
|---|---|
| 传输中网络断（含静默断链） | 空闲超时 ≤30s 显形，双方自动退避重连续传，用户无感 |
| 重连耗尽 | 发送端按原始路径报告成败清单（非零退出）；接收端窗口超时退出 |
| 磁盘不足 | 接收端整体拒绝（verdict=0 + reason），发送端终局不重试 |
| 源文件传输中被改 | 块哈希不符 → 重请求轮耗尽 → 干净失败并提示（终局，不重试） |
| 恶意对端诱导循环 | checkpoint ≤3 轮硬上限 |
| manifest 超限/重复 rel/首段 `.mtdss` | `verify` 拒绝 |
| Windows 保留名（CON 等）/尾点终名 | 最终 rename 干净失败（文档化限制，V1 不做名字转写）；重发不重传数据，仅复核重试改名 |
| STUN 单端口通告 | alt=primary 照常工作，不降级 |

## 11. 测试策略

1. **manifest**：多文件签名/验签往返、重复 rel 拒绝、首段 `.mtdss` 拒绝、上限校验；
2. **对账（intake 单元）**：无记录 / 部分 bitmap / done / 哈希不符四态的计划计算；记录写中断（截断/半写 JSON）→ 旧状态作废按 FULL 重传，不崩不误续；冲突改名后的 SKIP 判定走 `final_rel` 而非原始 rel（防重复传文件）；
3. **传输 e2e（loopback QUIC 直连，不经信令）**：多文件 + 目录结构落盘与冲突改名；同 manifest 二发全 SKIP 零字节；断连后重连只补缺块（按块流计数断言）；坏块注入（测试钩子丢弃一块数据）→ checkpoint 重请求补齐；FileDone 逐文件回；
4. **重连 e2e（cli 级，信令 + punch）**：中途 close 连接 → 自动续传完成、用户无感；**静默断链**（持有 Connection 的测试侧 drop 句柄不 close，模拟进程消失，亚秒级空闲超时注入）→ 两侧各自在超时内进入重连/等待窗口；不可恢复对端 → 小重试预算注入 → 失败报告文案含原始路径与成败清单；终局错误（Rejected）不消耗重试轮次；
5. **CLI**：多路径/目录/空目录/符号链接/rel 冲突/输入目录名为 `.mtdss` 各一例；
6. **债务**：单端口通告路径可用；`--stun-port` 校验负例；RFC1918 断言；clippy `--all-targets`；
7. **安全负例**：伪造块（file_idx 越界/offset 不在表/超长）拒收；谎报全 SKIP 只害收端自身（发送端报告按 FileDone 如实）。

## 12. 交付与验收

- 门禁：`cargo test --workspace && cargo clippy --workspace --all-targets -D warnings` 双平台 CI 绿；
- 验收清单：
  - [ ] 多文件/目录传输，结构还原，同名冲突自动改名
  - [ ] 隐藏收件区：`.part` 随完成即删、完成记录留存；杀进程式中断后手动重发跳过已完成/续传半程/新传未开始
  - [ ] 传输中断线自动重连续传（注入断连；静默断链经空闲超时同样触发）
  - [ ] 冲突改名后的文件在手动重发时仍正确 SKIP（`final_rel` 对账）
  - [ ] 重连耗尽后按原始路径报告成败清单；终局错误不空耗重试轮次
  - [ ] 调度：先小后大，4×4 并发常量可注入缩小
  - [ ] M2b 六笔债务清零
- 流程：worktree 分支 → PR 合入 main；随后启动 M2d（NAT 矩阵 + 混沌）。
