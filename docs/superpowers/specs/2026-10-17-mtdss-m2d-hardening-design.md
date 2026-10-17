# MTDSS M2d 设计文档：NAT 矩阵 + 混沌验收 + M2c 终审债务清偿

- 日期：2026-10-17
- 状态：待评审（brainstorm 对话定稿）
- 上游规范：[2026-10-15-mtdss-v1-design.md](2026-10-15-mtdss-v1-design.md) §5、§11.3、§11.4
- 子项目定位：M2a（债务清偿）→ M2b（NAT 打洞）→ M2c（传输 v2，已合并 d86068c）→ **M2d（收官硬化）** → M3（daemon + UI）

## 1. 目的与范围

M2c 交付了传输 v2 并过了终审，终审判定 7 笔 RIDE 债务全部带入 M2d；M2b 遗留的 netns NAT 矩阵与 netem 混沌（v1 §11.3/11.4 明确为 M2 重心）也在此轮落地。M2d = 让 M2「传输成熟」名副其实的收官里程碑。

| 交付 | 内容 |
|---|---|
| M2c 终审 RIDE 清偿 | ⓪-⑦ 共 8 笔 + punch.rs 注释 + 测试注入面/可观测性增补（§3） |
| NAT 矩阵（netns harness） | full-cone / restricted / port-restricted / symmetric 组合，CI 验证打洞成功/预期失败（§4） |
| netem 混沌 | 丢包/延迟/断网下传输与重连续传验收（§5） |
| srflx 实效裁定 | 矩阵内自动化断言 + 真机对传进发布检查单（§6） |

**范围外**：加密中继（V2）、对称 NAT 端口预测（V2+）、IPv6 候选策略（沿用 M1 v4 策略）、暂停/取消显式协议与多任务队列（M3 daemon）、跨会话自动恢复、Windows 本机跑矩阵/混沌（netns 与 netem 是 Linux 专属；Windows 侧只跑常规门禁）、接收目录双 recv 并发（M3）。

## 2. 核心裁定（brainstorm 定稿）

1. **矩阵 + 混沌全部进 PR 必跑门禁**：CI 增加 Linux 专用 job，与双平台 test/clippy 并列必绿。稳定性优先设计——每例硬超时、失败即转储双侧日志，宁慢勿抖。
2. **「真实 srflx 实效」两层验收**：自动化层 = 矩阵中的 NAT 是真 MASQUERADE 映射，srflx 候选拿到的就是真实外部地址，断言其出现且连接走通；真机层 = 一次跨真实网络的手动对传记录，进发布检查单，不进 CI。
3. **⑦ multi_path e2e 偶发挂起不阻塞交付**：诊断已在位（ff08a81 超时 kill + 转储子进程输出），M2d 期间对 harness 做一轮针对性排查；再现即按转储定位，查无则以「诊断在位、机制未定」结案。
4. **fsync 只加在完成临界路径**：每文件完成时一次有序落盘，周期性 bitmap 记录重写（16 块/2s）不 fsync——崩溃代价仍由既有 FULL 兜底吸收。

## 3. M2c 终审 RIDE 清偿

### ⓪ CI 门禁补齐（本次 recon 新发现）

[ci.yml](../../../.github/workflows/ci.yml) 的 clippy 步骤缺 `--all-targets`——M2c 起立的常态门禁实际未进 CI。修：该行补 `--all-targets`，矩阵 job（§4）同文件新增。

### ① intake 掉电安全（最优先）

**问题**：`.part` 改名终文件与记 `done=true` 都只到页缓存。`.part` 是预分配（set_len）文件，size 恒吻合，撕裂后对账的 size 检查失效 → 掉电可致「记录 done、终文件撕裂」且后续手动重发永远 SKIP（数据永久损坏，用户无感知）。

**修法（有序落盘，完成路径每文件一次）**：

```
fsync(.part 数据) → rename(.part → 终名) → fsync(终名直接父目录 与 .mtdss)
→ 写 .json.tmp + fsync → rename(.json.tmp → 记录) → fsync(.mtdss 目录)
```

- **fsync 的目录集合**：终文件**直接父目录**（顶层 rel = out_dir；目录发送的深层 rel = 其所在子目录——fsync out_dir 不能使深层 rename 持久，ext4 上必须 fsync 直接父目录）+ `.mtdss`。`create_dir_all` 出的中间目录不 fsync：目录项丢失使 SKIP 退化回 FULL 重传，落在既有接受窗口内。
- 不变式：**终文件 durable 先于 done 记录 durable**。修后崩溃窗口只剩 M2c 已接受的两个（数据未落盘 → 记录仍 partial → FULL/RESUME 兜底；已改名未记 done → FULL 重传一次），SKIP 粘连窗口消除。
- 目录 fsync 跨平台：Unix `File::open(dir).sync_all()`；Windows `OpenOptions` 加 `FILE_FLAG_BACKUP_SEMANTICS` 打开目录后 `sync_all()`。取不到句柄时降级为日志警告不失败（Windows 语义本就弱于 ext4，best-effort）。
- 周期性记录重写与 bitmap 批落盘不加 fsync（裁定 4）。

### ② 超大 manifest 拒绝路径双侧挂起

**问题**：接收端见 `man_len > 64 MiB` 立即拒绝，但不再读流；发送端 `write_all(man_bytes)` 在流控上永久饿死（读侧已放弃），双方挂住。

**修法**：cap 拒绝路径不等 EOF——发 `TAG_REJECT` 后对控制流执行 `stop_stream(错误码)`（而非 `finish`）。发送端 write 即刻以 STREAM_STOPPED 显形，映射为 `SendError::Rejected`（携带 reason，终局不重试），干净退出。

### ③ 计划帧总量上限

**问题**：单 bmp 已限 1 MiB、文件数已限 65535，但总量无界——恶意收端可回 65535 × 1 MiB 位图，发送端逐段解析无总闸。

**修法**：`recv_plan` 累计全部 bmp 字节数，超 `MAX_PLAN_TOTAL_BITMAP_BYTES` 即 `Protocol` 错误（发送端终局）。**取值 64 MiB（我的判断，对齐 manifest 量级）**：合法最坏情形 Σbmp ≈ 总字节量/8MiB，64 MiB 上限足够宽松且与既有界同量级。

### ④ cli 级静默断链 e2e（v1 §11.4 / M2c 设计 §11.4 欠账）

既有 e2e 覆盖 close 式断连与窗口行为；缺「静默死亡 + 缩短的 keep-alive/idle 注入」的 cli 级用例。补两个方向：**杀发送方**（传输中硬杀真实 `mtdss send` 子进程——OS 层收走 socket、零 QUIC 关闭帧，即真静默死亡；quinn 的句柄 drop 会发隐式 CONNECTION_CLOSE，模拟不了）→ 断言接收端在注入的空闲超时内显形、窗口到点退出；**杀接收方**（硬杀 `mtdss recv` 子进程后 2s 重启）→ 断言发送端在空闲超时内检测、退避重连、对重启后的接收端经 `.mtdss` 对账续传完成。

- ~~发送侧故障钩子~~（计划期裁撤：真实子进程硬杀严格强于进程内钩子，且进程内无法模拟静默死亡）。
- 注入经 ⑨ 的 env 面；值取「缩短但留裕量」（如 idle 1s 级、断言窗口 ≥5× 超时），双平台 lane 都跑（与 M2c 既有亚秒级重连 e2e 同类，非 ⑦ 那种无界挂起型）。

### ⑤ TransportTuning 毫秒截断 debug_assert

亚秒级 keep-alive/idle 配置经 quinn `TransportConfig` 毫秒截断可能归零（keep-alive 失效或 idle 无限）。注入处加 `debug_assert!`：截断后任一侧为 0 即 panic 于测试期。

### ⑥ collect_items 误入 `.mtdss` 子目录

目录递归收集遇名为 `.mtdss` 的子目录会当载荷转发（对端 verify 只拒**首段** `.mtdss`，深层的会照收——终名可落进收件隐藏区结构里）。修：递归中遇 `.mtdss` 目录项跳过 + 警告（与发送预检、对端首段拒绝三层对称）。

### ⑦ multi_path e2e CI 偶发挂起（2026-10-17 ubuntu 首跑）

按裁定 3 处理：一轮针对性 harness 排查（recv 子进程等待逻辑、stdout 管道、信号路径）；不阻塞交付，查无即结案。

### ⑧ punch.rs 预算边界单节拍残竞态注释

M2b 遗留小账：下次编辑 punch.rs 时在预算边界处补注释说明残竞态性质。本轮顺带清偿。

### ⑨ 测试注入面与可观测性增补（评审补入；④/§5/矩阵的公共前置）

- **CLI 测试专用环境变量**：真实二进制现在只认 `MTDSS_HOME`，没有任何调参面；矩阵/混沌跨 netns 只能跑真实进程。新增 `MTDSS_TEST_*` 系列环境变量（idle/keep-alive 毫秒、重试预算与退避、接收窗口、punch 预算——按用例需要逐个加，不一次铺全），读入集中在 cli 入口 → flow 调参处；文档明标「测试专用、不稳定、随时可删」，不进用户文档。
- **候选日志行**：gather 完成后输出一行稳定格式日志（如 `candidates: local=[a,b] srflx=[c] symmetric=false`），矩阵 ✅ 用例 grep 它断言 srflx = 网关外地址。
- **punch_err 恒带本侧 NAT 类型**：现在仅本侧对称时才附加提示；改为错误文案恒含本侧 NAT 类型与候选数（❌ 用例两侧输出都断言得上，用户侧诊断同步受益）。

## 4. NAT 矩阵（netns harness）

### 4.1 拓扑

```
ns: pub ── 信令服务器 + STUN 双端口（"公网"侧）
 │ veth
ns: gwA ── iptables NAT（按用例模拟类型 A）
 │ veth
ns: a ─── mtdss send（真实二进制）
（B 侧对称：gwB + b，b 跑 mtdss recv）
```

- 五个 netns：`pub / gwA / a / gwB / b`，veth 直连，默认路由指向各自网关；gw 开转发。
- NAT 类型模拟（iptables + conntrack，规则集细节归计划）：
  - **full-cone**：MASQUERADE 出向 + 入向对已建映射无来源限制（DNAT 洞常开）；
  - **restricted-cone**：入向放行条件 = 源 IP 曾被本机联系（不限端口）；
  - **port-restricted**：conntrack `ESTABLISHED` 状态默认行为；
  - **symmetric**：按目的分桶的 SNAT 端口分配（不同目的 → 不同外部端口）。
- STUN/信令在 `pub`，两侧 peer 经各自 NAT 访问——srflx 即网关外地址，为真实映射。

### 4.2 矩阵与预期结果

10 个无序类型组合、共 **14 例**（6 个纯锥组合 ×1 + 4 个含对称组合 ×2 键序）。预期由互拨模型（小键优选拨号、大键侧非首选臂仍拨且 held 于轮末交付）推出，**与键序无关**；含对称组合双键序各跑一次以回归此性质：

| A＼B | full | restricted | port-restr. | symmetric |
|---|---|---|---|---|
| **full** | ✅ | ✅ | ✅ | ✅ |
| **restricted** | | ✅ | ✅ | ✅ |
| **port-restr.** | | | ✅ | ❌ 预期失败+诊断 |
| **symmetric** | | | | ❌ 预期失败+诊断 |

- ❌ 用例断言：限时报错（预算 10s 内）、文案含对称 NAT 提示与候选数；
- ✅ 用例断言：连接建立、多文件传输完成且哈希吻合、**两侧日志出现等于各自网关外地址的 srflx 候选**（⑨ 的候选日志行，§6 自动化实效断言）。
- 私有网候选跨网关不可达属正常（dial 并行烧超时，预算内），不计失败。

### 4.3 harness 形态与 CI

- **bash 脚本** `scripts/nat-matrix/`（我的判断：ns 编排 shell 最简；Linux 任意环境可本地复跑），按用例建拓扑 → 跑真实 `mtdss`/信令/STUN 二进制（release 构建一次）→ 断言退出码与日志 → 清理 ns。
- 每例硬超时 + 失败时转储两侧全部输出（对齐 ⑦ 的诊断风格）；脚本幂等可重入。
- CI：ci.yml 新增 `nat-matrix` job（ubuntu，sudo 建 ns），**PR 必跑必绿**；矩阵 14 例 + 混沌 4 例（失败例烧满 10s 打洞预算，其余秒级）合计预估 ≤12 分钟，编译缓存复用。

## 5. netem 混沌（同 harness，tc netem 注入 gw 侧）

| 用例 | 注入 | 断言（时间断言 = 每例硬超时内完成） | 硬超时初值 |
|---|---|---|---|
| 高丢包 | 双向 loss 10% | 传输完成、哈希吻合 | 120s |
| 高延迟 | 双向 delay 200ms±50ms | 传输完成（慢而不挂） | 180s |
| 中途断网 | 传输中 `ip link` down 3s 后 up（⑨ env 缩短 idle 至 1s 级，确保走真重连路径） | 自动重连续传完成、终态完整 | 120s |
| 永久断链 | 传输中 down 不恢复（⑨ env 小重试预算 + 短窗口） | 发送端耗尽 → 失败报告含原始路径清单；接收端窗口超时退出 | 120s |

- 载荷：多文件 4 个合计约 12 MiB（够触发中途故障，不撑时长）。时间断言一律以每例硬超时为界（不做紧界，「宁慢勿抖」），初值如上，收尾时按 ≥3× 实测中位校准后才算定稿。
- 调参一律经 ⑨ env 面注入（跨 netns 的真实进程没有函数参数可传）；拓扑固定 port-restricted×port-restricted（对混沌最敏感的常见真实形态）。
- 「永久断链」覆盖 v1 §11.4「断网验证续传」的失败半边；成功半边由「中途断网」覆盖。

## 6. srflx 实效与发布检查单

- **自动化（进矩阵）**：✅ 用例断言 srflx 候选 = 网关外真实地址且连接走通（§4.2）。
- **真机（不进 CI）**：发布检查单新增条目「跨真实网络对传一次（两处不同 NAT，如家宽×手机热点），记录 NAT 类型预判与结果」。V1 发布前执行一次，记录归档 docs。检查单本身随本里程碑建立（docs/release-checklist.md）。

## 7. 错误与诊断（增量）

| 场景 | 行为 |
|---|---|
| 掉电/崩溃于完成临界区 | 终文件与 done 记录有序落盘；SKIP 判定不再可能指向撕裂终件 |
| 对端 manifest 超限 | 收端 stop_stream + reject reason；发端 Rejected 终局，不挂不重试 |
| 恶意计划帧（总量超限） | 发端 Protocol 终局 |
| 打洞失败（对称×端口受限/对称×对称） | 预算内报错，文案含 NAT 类型、候选数、建议 |
| 静默断链（进程消失/NAT 重绑） | 空闲超时显形 → 重连续传（cli 级 e2e 验收） |

## 8. 测试策略

1. **单测/集成（既有门禁内）**：intake fsync 序列（以可注入的 sync 抽象或特征 mock 断言调用次序与目录集合 = 直接父目录 + `.mtdss`）；cap 拒绝路径两侧不挂（loopback QUIC）；计划帧总量超限 Protocol；collect_items 跳过 `.mtdss`；debug_assert 触发负例；⑨ env/候选日志/punch_err 文案的解析与格式测试。
2. **cli e2e（既有套件增量）**：④ 静默断链用例（双平台 lane）；⑦ 排查后如需加固 harness 一并提交。
3. **矩阵 + 混沌（新 CI job）**：§4/§5 全表。
4. 门禁回归：ci.yml clippy 补 `--all-targets` 后双平台全绿（⓪）。

## 9. 交付与验收

- 门禁：`cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings` 双平台绿 **且** `nat-matrix` job（矩阵 + 混沌）PR 必绿。
- 验收清单：
  - [ ] RIDE ⓪-⑦ 全部清偿，⑧ 注释落位，⑨ 注入面/日志/文案增补交付
  - [ ] 矩阵 10 组合 ×（对称相关 ×2 键序）预期结果全对；❌ 用例诊断文案符合 §4.2
  - [ ] 混沌 4 用例断言全过
  - [ ] srflx 自动断言进矩阵；发布检查单建立并含真机对传条目
  - [ ] 掉电语义：完成临界区有序落盘（测试可证）
  - [ ] ⑦：排查有结论或「诊断在位」结案记录
- 流程：worktree 分支 → PR 合入 main；随后启动 M3（daemon + UI）设计。
