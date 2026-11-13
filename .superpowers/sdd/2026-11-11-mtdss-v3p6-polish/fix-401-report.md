# fix-401:fetch_devices 401 就地轮换重试(长连跨 24h 令牌过期自愈)

- **Commit**: `570eaae` @ `v3p6`(PR #30)
- **日期**: 2026-11-13
- **波次**: V3-6 field bug 返工(实机早间复现:job 起手 `GET /devices` 401)

## 1. 缺陷与根因(实机证据)

访问 JWT TTL = 24h(`server/signaling/src/auth.rs` mint_access_jwt);令牌轮换是
**纯被动**面:仅信令 WS 重连被拒(hub.rs AuthDenied)时才 refresh。服务器只在
WS 握手验签,长连不重连就永不轮换——跨过 24h 边界后,会话 JWT 变陈,下一次
job 起手的 REST 拉目录 `GET /devices`(Bearer)401,任务败于
`list devices: HTTP status client error (401 Unauthorized)`。实机 tasks.json
今晨 09:03 一例 + 11-06 一簇同签名,每逢隔夜任务必发。

## 2. 实际落地的设计(controller ruling 的落地形态)

### 2.1 单一收口:双份重复实现先归一

修前 `fetch_devices` 有**两份逐字节等价实现**:

- `crates/daemon/src/flow.rs:69`(send/ctrl/api 四个产线调用点在用——**实机
  失败栈走的就是这一份**)
- `crates/daemon/src/account.rs:531`(hub.rs 快照在用)

若只修 account 一份,flow 那份(即失败现场)漏网;两份同修即埋分叉。
处置:**flow 的重复实现删除**,REST 面统一收口在 `crate::account`
(flow.rs 留注释说明收口位置与动机;flow 的 `http_client` 保留,verify 面仍用)。

### 2.2 401 可检测(文案逐字不变)

`account.rs` 新增私有标记错误 `DevicesUnauthorized { source: reqwest::Error }`
(`Display = "list devices"`,reqwest 错误作 `source`)。`fetch_devices` 先判
`status == 401` 再 `error_for_status`,401 时返回
`anyhow::Error::new(DevicesUnauthorized { source })`:

- 链根 = 标记 → `downcast_ref::<DevicesUnauthorized>()` 可辨;
- `{:#}` 渲染 = `list devices: HTTP status client error (401 Unauthorized)
  for url …` —— 与修前**逐字节一致**(标记 Display 即原 context 文案,
  source 保 reqwest 原文),日志/错误面零漂移;
- 非 401 路径维持原 `error_for_status().context("list devices")` 不动。

### 2.3 自愈包装:`fetch_devices_with_refresh`

```rust
pub(crate) async fn fetch_devices_with_refresh(
    s: &Session,
    persist: impl Fn(&Session) -> Result<()>,
) -> Result<Vec<serde_json::Value>>
```

三分支 + 恰一次语义(与 hub.rs AuthDenied 处置同口径):

1. 非 401 错误 → **原样透传**,绝不轮换;
2. 401 → `account::refresh` 轮换**恰一次** → clone Session 换新 jwt/refresh →
   `persist(&rotated)` 写回(落盘失败仅告警,**沿用内存令牌继续重试**——
   hub 落「沿用旧会话重试」,REST 面新令牌已到手,弃之才是浪费)→ 用
   rotated 令牌**重试恰一次**(无循环);
3. 401 且轮换失败 → 浮出**原始 401**(带告警注明轮换失败),轮换错误绝不
   遮掩原始排障面,重试不发生。

`persist` 参数(而非直接依赖 DaemonState)使 account.rs 保持零状态依赖,
三分支可用回环 axum 服务器单测(见 §4)。

### 2.4 产线接线(五个调用面全过包装,persist 统一 `state.update_session`)

| 调用面 | 位置 | persist |
| --- | --- | --- |
| send 驱动扫描(**实机失败点**) | flow.rs `run_send_with`(原 :733) | `state.update_session` |
| devices RPC 现拉 | api.rs `devices`(原 :230;5s 超时口径不变,自愈多一趟 refresh 往返,超时照旧回退缓存快照) | `state.update_session` |
| ctrl 池 list 前拉目录 | ctrl.rs(原 :1164) | `state.update_session` |
| hub 全量快照(WS 建连后) | hub.rs `refresh_snapshot`(原 :1021) | `state.update_session` |
| CLI `devices_cmd` | flow.rs(**签名加 `home: &Path`**;全仓无调用者,CLI 实际走 IPC→api 面,签名改法零影响) | `rotated.save(home)`(与 `update_session` 的落盘段同语义;**不落盘会烧掉已消费的 refresh token**,故不给无参透传口) |

hub.rs 面(ruling:「wiring too is fine/uniform if trivial」)接线是平凡的
(state 已在手),按 uniform 接线,零行为差(紧随握手令牌新鲜,自愈多半
不触发)。

## 3. 测试(TDD)

### 3.1 单测:三分支 + 落盘语义,回环 axum 服务器逐字可见

account.rs tests(沿用房式 `request_code_tolerates_echo_off` 的本地 axum
回环模式;代理豁免提取共享 `loopback_no_proxy()`——全模块一把 Once,消除
原用例内联 Once 与新用例并发 set_var 的竞态面):

1. `wrapper_rotates_on_401_persists_and_retries_once`(**分支②**):首趟
   401 → 轮换恰一次(refresh 命中数=1)→ persist 恰一次(rotated 会话,
   server/device_id 原样保留)→ 重试恰一次(fetch 命中数=2),Authorization
   头序列 `["Bearer stale-jwt", "Bearer rotated-jwt"]` 逐字断言;
2. `wrapper_passes_through_non_401_without_rotation`(**分支①**):403 原样
   Err(文案带 `list devices`/`403`),不轮换、不落盘、不重试;
3. `wrapper_surfaces_original_401_when_refresh_fails`(**分支③**):refresh
   面 401 → 返回原 401(文案含 `list devices`+`401`,不含 `auth/refresh`),
   标记仍在链根(`downcast_ref` 可辨),fetch=1(不重试)、不落盘;
4. `wrapper_retries_with_rotated_token_even_if_persist_fails`(hub 同语义
   钉死):persist 返回 `Err("disk full")` → 自愈不折断,重试仍带 rotated
   令牌,persist 调用数=1。

另:`devices_cmd` 落盘语义(轮换必须落盘,否则烧掉已消费的 refresh token)
在 §2.4 表内钉死;`Session::save` 即 `update_session` 所用同一落盘函数。

### 3.2 测试输出

- 单步 red:`fetch_devices_with_refresh` 未实现时 5 处 E0425 编译失败(TDD
  先行落测);实现后 lib 229 用例全绿。
- **全量**:`cargo t -p mtdss-daemon -j 2 --no-fail-fast`(env unset:
  http_proxy/https_proxy/HTTP_PROXY/HTTPS_PROXY/MTDSS_UPDATE_URL/
  MTDSS_SIGNALING_URL):

  ```
  22 个目标全绿,397 passed; 0 failed
  (lib 229 + daemon_e2e 32 + jobs_e2e 26 + relay_e2e 23 + login_v3p1 17
   + ctrl_e2e 14 + online 12 + login 10 + v3p1_e2e 6 + photo_serve_e2e 6
   + inbox_* 6+4+3 + contract_golden 1 + 其余 0 用例目标)
  ```

  注:首跑(fail-fast)中 `receiver_restart_progress_monotonic_across_resume`
  (kill-resume 时序敏感用例)在整包并发负载下超时 FAIL;**单测隔离复跑
  PASS(74s < 180s 预算)**,且 `--no-fail-fast` 全量二跑(含该用例)PASS
  ——判定为负载时序 flake,与本修无关(非 401 路径行为逐字节不变)。
- `cargo clippy -p mtdss-daemon --all-targets`:零警告(途中一记
  `empty_line_after_doc_comments` 由 flow.rs 收口注释改普通注释修掉)。

## 4. 不变量与边界(记录在案)

- **文案稳定**:401 错误渲染逐字节不变(`list devices: HTTP status client
  error (401 …)`),识别面纯内部 downcast。
- **重试恰一次**:无循环;第二次 401 原样上抛(等 WS 面的连续拒收口)。
- **不落盘不轮换危险面**:`devices_cmd` 若给无落盘的透传口,会消费旧
  refresh token 而不存新对——单次即烧会话。落盘(home 参数)是硬要求,
  已在实现与注释钉死。
- **api.rs 5s 超时**:自愈路径(401→refresh→重试)最坏两趟 REST + 一趟
  refresh;超时即按既有口径回退缓存快照,已落盘的轮换不白做(下个窗口
  生效)。
- **跨面竞态**:WS 面与 REST 面可能各自轮换(双方都见过期 401);后到者
  用已消费的旧 refresh 会失败 → 浮出原始 401,不遮掩、不烧会话(state 中
  已由先到者写入新令牌)。与 hub 现行被动轮换的并发语义一致。

## 5. 文件清单

- `crates/daemon/src/account.rs` — 401 标记 + `fetch_devices_with_refresh`
  + 四分支单测(+代理豁免共享化)
- `crates/daemon/src/flow.rs` — 删重复 `fetch_devices`;send 驱动与
  `devices_cmd`(+home 参数)接线
- `crates/daemon/src/api.rs` — devices RPC 接线
- `crates/daemon/src/ctrl.rs` — ctrl 池接线
- `crates/daemon/src/hub.rs` — `refresh_snapshot` 接线
