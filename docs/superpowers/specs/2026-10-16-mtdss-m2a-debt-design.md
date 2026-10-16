# MTDSS M2a 设计文档:债务清偿

- 日期:2026-10-16
- 状态:已评审(brainstorm 对话定稿)
- 上游规范:[2026-10-15-mtdss-v1-design.md](2026-10-15-mtdss-v1-design.md) §12
- 子项目定位:M2 = M2a(债务清偿)→ M2b(NAT 打洞)→ M2c(传输 v2)→ M2d(NAT 矩阵 + 混沌验收)。本文件只覆盖 M2a。

## 1. 目的与范围

M1 骨架贯通时搁置了四项硬性债务。它们全部是 M2 后续工作的地基,必须先行落地:

| # | 债务 | 为什么必须先落 |
|---|---|---|
| 1 | `signable_bytes` 长度帧化 v2 | 签名输入字节有歧义;多文件(M2c)使歧义加剧;且必须先于任何中继模式 |
| 2 | 信令面 TLS | 部署前置:公网明文 HTTP/WS 不可上线 |
| 3 | WS keepalive | 半死连接不可感知:presence 脏数据 + 客户端信令等待永久挂起风险 |
| 4 | 传输流式化(接收方) | 接收方整文件驻内存;大文件直接 OOM;多文件(M2c)的前置 |

**范围外**:多文件/目录、断点续传(M2c);NAT 打洞(M2b);manifest.proto 消息结构变化(本项只改签名输入字节,proto 不动)。

## 2. 签名帧化 v2

### 2.1 现状与缺陷

`crates/core/src/protocol/manifest.rs::signable_bytes` 把各字段直接串接,无长度前缀:

```
transfer_id || rel_path || size_le || sha256 || (chunk.sha256)*
```

`rel_path` 与后续字段之间没有边界,不同清单可产生相同签名字节(例:files=["ab","c"] 与 ["a","bc"] 在特定 size/hash 对齐下)。单文件 + 64 字符 hex transfer_id 下实际可利用性低,但多文件与清单结构扩展会放大攻击面,且签名协议应当无歧义。

### 2.2 v2 格式(定长大端长度前缀 + 域分隔)

```
signable_bytes_v2 =
    b"MTDSS-MAN-SIG-V2"                        // 16B 域分隔标签
 || u32 BE len(transfer_id) || transfer_id      // 长度按字节计
 || u32 BE file_count
 || 逐文件:
        u32 BE len(rel_path) || rel_path
     || u64 LE size                             // 定宽
     || sha256                                  // 定宽 32B
     || u32 BE chunk_count
     || 逐块:
            u64 LE offset                       // 定宽
         || u64 LE length                       // 定宽
         || sha256                              // 定宽 32B
```

设计要点:

- **域分隔标签**防跨协议签名重用(同一设备身份密钥还签信令体、将来签中继/punch 消息)。
- 每个变长字段(`transfer_id`、`rel_path`)长度前缀;`file_count`/`chunk_count` 消除"下一块 vs 下一文件"结构歧义;定宽字段不额外加前缀。
- 所有长度前缀 u32 BE;数值沿用 v1 的 LE(与现有 `to_le_bytes` 一致,减少无谓变更)。

### 2.3 版本与兼容

- `MANIFEST_VERSION` 1 → 2;`build_single` 写 2,`verify()` 仅接受 2(既有分支已能对非匹配版本返回 "unsupported manifest version")。
- **硬切,不做 v1 兼容**:无已发布客户端;M1 旧端在 v2 端处得到干净报错。
- 只保留单一实现(单一 `signable_bytes` 函数),不留 v1 代码路径。

### 2.4 测试

- 黄金字节向量:固定输入(多文件、含多字节 UTF-8 文件名)断言完整签名字节。
- 歧义回归:构造在 v1 串接格式下字节相同的两个清单,断言 v2 字节不同。
- 既有 manifest 单测(构建/验签/篡改拒绝)全量回归,适配版本号。

## 3. 信令面 TLS

### 3.1 服务端

- **仅 TLS 单端口**(默认 8787 不变),不再监听明文。rustls 终止于信令服务器进程内(axum 服务循环包 TLS acceptor;具体承载 crate——手写 tokio-rustls accept 循环或 axum-server——由实现计划定)。
- 参数:`--tls-cert <pem> --tls-key <pem>` 装载证书与私钥;缺失即启动失败并给出明确报错。
- `--dev-gen-tls <dir>`:用 rcgen(workspace 已有依赖)生成自签 CA + 服务端叶子证书,写出 `ca.crt` / `server.crt` / `server.key`(0600,dir 须为空或不存在)后退出。供开发与 CI 使用,并打印 `--ca <dir>/ca.crt` 提示。

### 3.2 客户端(CLI)

- `Session` 结构增加可选 `ca: Option<PathBuf>` 字段(session.json 持久化):
  - `register --ca <path>` 时写入;
  - 其余命令默认用 session 内值,命令行显式 `--ca` 覆盖。
- **REST(reqwest)**:启用 `rustls-tls` feature(当前 `default-features=false` 连 TLS 后端都没有);有 ca 时 `add_root_certificate`,无 ca 走内置 webpki 根(生产域名证书场景)。
- **WS(tokio-tungstenite)**:有 ca 时构造含自定义根存储的 rustls `ClientConfig`,经 `Connector::Rustls` 连接;无 ca 保持现状。`ws_url` 的 http→ws / https→wss 前缀映射已就绪,不动。

### 3.3 测试

- 服务端既有单测(auth/devices/ws)保持在 router 级直测,不经 TLS 层。
- 集成:dev-gen-tls 生成 → 以 TLS 启动服务器 → CLI `--ca` 完成 register/devices/send/recv 全链路(https + wss)。
- 负例:错 CA(不信任)连接必须失败。

## 4. WS keepalive

对称双向,WS 标准 Ping/Pong 帧,无需协议协商,时长为代码常量(可注入以便测试):

- **服务端**(`handle_socket` 主循环加 interval 臂):每 **20s** 发 `Ping`;跟踪最后入站时刻,任一入站帧(Pong/Relay 等)刷新;**60s** 无入站 → 断开连接,走既有 presence 下线清理路径。
- **客户端**(`SignalingClient` 内部任务):命令通道从 `Vec<u8>` 改为枚举 `{ Binary(Vec<u8>), Ping }`,每 **20s** 排一个 Ping;读侧跟踪最后入站,同一连接 **60s** 静默 → 发出 `ClientEvent::Closed` 并退出任务。
- 两侧对收到的 Ping 由 tungstenite/axum 在持续读取时自动回 Pong(双方读循环均常驻,满足自动回帧条件)。
- 时长参数(interval、静默上限)以参数/配置注入:生产常量 20s/60s,测试用亚秒级。

### 4.1 测试

- 服务端:测试常量下,静默客户端(只握 Hello 不再发帧、不回 Pong)被断开且 presence 下线。
- 客户端:测试常量下,静默服务器触发 `ClientEvent::Closed`。
- 回归:正常收发不受 keepalive 干扰(既有 ws 测试全量通过)。

## 5. 传输流式化(接收方)

### 5.1 现状与缺陷

`recv_files` 把所有已到块缓存在 `BTreeMap<u64, Vec<u8>>`,全块到齐后一次性写出并顺带算整文件哈希——接收方内存 = 整文件大小。发送方已逐块读取(每块 ≤1 MiB),不动。

### 5.2 新接收路径

1. 打开 `.part` 后 `set_len(size)` 预分配(NTFS/ext4 稀疏扩展,不实际写零遍历)。
2. 每收到一块:**先验块哈希**(防坏块写盘),再 `seek(offset)` + `write_all` 定位落盘;内存 O(1 块)。
3. `pending`/`remaining` 记账逻辑保持(只收清单内 chunk_id、防重放同块)。
4. 全块到齐后:**顺序读回** `.part` 流式计算整文件 SHA-256,与清单比对。
5. 通过后沿用既有 `unique_target` 原子改名落盘。

设计取舍:读回是第二遍顺序读(磁盘缓存命中高、顺序带宽大),换取实现简单与内存恒定;否决"有界乱序窗口单遍写 + 顺序哈希更新"方案(复杂度高、乱序大时仍需落盘缓冲,收益不成比例)。

### 5.3 测试

- 16 MiB 随机数据正确传输(覆盖多块定位写 + 读回校验)。
- 既有传输测试(含坏块拒绝、空间预检、断连清理)全量回归。
- 内存占用由实现保证(定位写、无全文件缓冲),不做运行时断言。

## 6. 交付与验收

- 门禁:`cargo test --workspace && cargo clippy --workspace -- -D warnings` 双平台 CI 绿(既有 ci.yml)。
- 验收清单:
  - [ ] v2 签名格式落地,黄金向量 + 歧义回归测试通过,`MANIFEST_VERSION=2` 硬切
  - [ ] 信令服务器 TLS-only,`--dev-gen-tls` 可用,CLI `--ca` 全链路 https/wss e2e 通过,错 CA 负例通过
  - [ ] keepalive 双向生效,静默对端被判定死亡(测试常量下),presence 正确清理
  - [ ] 接收方 16 MiB 文件传输正确,代码中无整文件内存缓冲
- 全部落在独立 worktree 分支,PR 合入 main 后 M2a 关闭,随即启动 M2b(NAT 打洞)设计。
