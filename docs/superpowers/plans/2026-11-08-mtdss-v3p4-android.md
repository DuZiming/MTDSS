# MTDSS V3-4 安卓重构 + 跨端对齐 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 安卓 App 按 iOS v1(含 V3-3 终态)整体重写(架构/操作/交互/视觉 1:1),一次装入对齐八项 + Windows 四项 + 扫码器 + 快捷目录落位 + 安卓服务桥。

**Architecture:** 引擎先行(`roots.set` 热注册 + 服务桥 = 照片桥模式泛化),安卓 UI 按「主题 token 层 → 四标签壳 → 各 tab → 设置/扫码/分享 → 应答器接线」重建(旧屏共存到 T16 退役),Windows 新建设置对话框消费同一 `roots.set`。

**Tech Stack:** Rust(crates/daemon)+ Kotlin/Compose(纯 JVM 测试面)+ Tauri/TS(ui crate)+ golden JSON 三端防漂移。

**Spec:** `docs/superpowers/specs/2026-11-08-mtdss-v3p4-android-design.md`(已批准,2026-11-08;执行者每任务前必读对应节)

## Global Constraints(全任务隐含)

- 视觉规格 = spec §2 逐值:行高 58dp/名 17sp/副文 12.5sp/图标槽 30dp(蓝夹 23dp/灰问号文档 21dp)/展开钮图标左侧旋转 90°/触区 28dp/缩进 18dp/级/选圈 24dp(列 20→56dp)/分割线 0.5dp 内缩 20dp+缩进/搜索框 40dp r11/胶囊浮动 16dp r30。
- 配色:#FFFFFF / #007AFF / #F0F0F3 / #F2F2F7 / #8E8E93 / #1C1C1E / 红 #FF3B30;深色 #0D0F15→#12141C / #4E9AFF;语义色恒定:接收 #007AFF、发送 #34A853、调度 #8E8AA0、完成 #34A853、失败 #FF3B30。
- 默认全关:删除门 / 显示隐藏 / Windows 信任模式(**不调用 roots.set 或空集 = 守卫直通,桌面行为逐字节不变**)。
- fdpass 通道纪律:0x01 request_source / 0x02 接收 / 0x04 dest_query / 0x05 ensure_dir / 0x06-07 照片;本波新增 0x08 serve_list / 0x09 serve_export / 0x0A serve_delete。
- minSdk 29;胶囊 blur 仅 API 31+(<31 退化不透明,布局不变);照片权限:API 33+ READ_MEDIA_IMAGES+READ_MEDIA_VIDEO,29-32 READ_EXTERNAL_STORAGE。
- proto 增量恒走缺省值纪律(protoc 3.12);golden 单一真源,三端同步;daemon/CLI 名恒 mtdss,UI 名恒 Synshu。
- 中文文案与 iOS 端逐字一致(「N 项」「A → B」「审批码 N」「转发 N 项到…」「删除文件夹及内容?」等)。
- 测试口径:cargo(本机 Windows 全量 + WSL `MSYS_NO_PATHCONV=1 wsl.exe env CARGO_TARGET_DIR=/root/t2-target /root/.cargo/bin/cargo t …`)/ gradlew `gradlew.bat :app:testDebugUnitTest` / ui `npm test`+`tsc`+`build`。
- 分支 `v3p4`(worktree 执行);冒烟真机 = MIX 3(Android 10,MIUI 语义库见项目记忆)。

## Review Focus(spec 隐含、无任务测试覆盖则最易咬人的五类;每条已钉到归属任务)

1. **roots.set 空集/未调用 = 直通零回归**(老客户端与桌面默认面)→ T1 步骤「空集直通回归测试」。
2. **vpath 逃逸形态**(`/收件箱/../x` 规范化后出集、`/收件箱-x` 字节前缀不匹配)→ T2 步骤「逃逸矩阵测试」。
3. **桥不可达**(应答器死/超时)→ 虚拟根隐藏、浏览诚实失败、不挂死(超时即摘,迟到应答拒)→ T2 步骤「不可达降级测试」。
4. **旧徽章色残留**(调度橙 #EF6C00 / upload 蓝 #1E88E5 任何一处漏换)→ T8 步骤「grep 门禁」。
5. **选择模式确认 = 只退出无副作用、0 选 = 退出** → T7 步骤「确认语义测试」。

---

### Task 1: `roots.set` IPC 热注册 + RootGuard 可换(引擎)

**Files:**
- Modify: `crates/daemon/src/roots.rs`(RootSpec/RootSource;roots_listing 按 visible 过滤)
- Modify: `crates/daemon/src/state.rs`(guard 持有改原子可换;`set_roots`)
- Modify: `crates/daemon/src/api.rs`(op 分派表 ~L102 邻域加 `"roots.set"`)
- Test: `crates/daemon/src/roots.rs`(内嵌 tests)+ `crates/daemon/tests/` 既有 e2e 邻域

**Interfaces:**
- Produces(engine 内稳定面,后续任务消费):
  - `pub(crate) struct RootSpec { pub name: String, pub path: String, pub source: RootSource, pub visible: bool }`
  - `pub(crate) enum RootSource { Fs, Bridge }`
  - `state.set_roots(specs: Vec<RootSpec>)`(原子换守卫 + 桥根登记表;空集 = 直通 None 语义)
  - IPC:`roots.set {"roots":[{name,path,source:"fs"|"bridge",visible}]}` → `{"ok":true}`;非法根丢弃留痕 fail-closed(沿 RootGuard::new 语义)
  - 守卫 `check`:`source=fs` 按真实路径前缀;`source=bridge` 按注册 vpath 前缀(词法规范化同款);对端根列表 = visible 条目

- [ ] **Step 1: 写失败测试**(roots.rs tests):`set_roots` 后 check 翻转(fs 根集内 ok/集外 err)、visible=false 条目不出现在根列表、空集后 `enabled()==false` 直通
- [ ] **Step 2: 跑测试确认失败** Run: `cargo test -p mtdss-daemon roots` Expected: FAIL(编译错,set_roots 未定义)
- [ ] **Step 3: 实现**:state guard 持有改 `Arc<ArcSwap-或-RwLock<RootGuard>>`(沿 state.rs 既有共享模式择一,保持 `state.guard()` 消费面签名不变);roots.rs 加 RootSpec/visible 过滤;api.rs 挂 op(参数解析沿 kv_put 风格)
- [ ] **Step 4: 跑测试确认过** Run: `cargo test -p mtdss-daemon` Expected: PASS(含既有 roots 矩阵零回归)
- [ ] **Step 5: e2e**:daemon e2e(既有 harness)加一例:IPC roots.set(fs 根)→ ctrl List "" 见该根 → 集外路径 INVALID_PATH
- [ ] **Step 6: Commit** `feat(daemon): roots.set 热注册与守卫可换(V3-4 §4.1;空集直通零回归)`

### Task 2: 服务桥引擎侧(对端浏览/下载/删除安卓内容)

**Files:**
- Create: `crates/daemon/src/serve.rs`(ServeBridge;逐条注释对齐 photo.rs 体例)
- Modify: `crates/daemon/src/ctrl.rs`(List/Stat/根列表:bridge 根路由)
- Modify: `crates/daemon/src/jobs.rs`(下载/编排源收集:bridge vpath 走 list+export)
- Modify: `crates/daemon/src/ctrl.rs`(delete:bridge vpath → fs.serve_delete)
- Modify: `crates/ipc/src/fdpass.rs`(通道常量 0x08/0x09/0x0A,沿既有 0x06/0x07 位)
- Test: `crates/daemon/src/serve.rs`(内嵌)+ `crates/daemon/tests/`(fake 应答器 e2e)

**Interfaces:**
- Consumes: T1 `RootSpec{source:Bridge}` 登记表(state 持 `Option<Arc<ServeBridge>>`,任一 bridge 根注册即建)
- Produces:
  - 事件契约(daemon→App,App 经 fdpass 回帧):`fs.serve_list {"root","sub","request_id"}` → `{entries:[{name,is_dir,size,mtime_ms,children?}],ok,truncated}`(0x08);`fs.serve_export {"root","sub","request_id"}` → `{ok,path}` App 写原始字节到 `{home}/serve-tmp/` 引擎读后 unlink(0x09,photo 同款);`fs.serve_delete {"root","sub","request_id"}` → `{ok,deleted}`(0x0A)
  - `ServeBridge::list(root,sub)` / `export(root,sub)` / `delete(root,sub)`,request_id 登记先于广播、超时即摘(photo.rs 同款;超时常量 LIST 10s / EXPORT 60s / DELETE 10s,`MTDSS_TEST_SERVE_TIMEOUT_MS` 注入口)
  - 路由规则:对端路径命中任一注册 bridge vpath 前缀(词法规范化后整段判定)= 桥面;写/移/复制经桥一律拒(只读来源 + delete 唯一例外);`/收件箱/../x` 规范化后出集

- [ ] **Step 1: 写失败测试**(serve.rs tests + roots 矩阵扩展):逃逸矩阵(`/x/../y`、字节前缀 `/收件箱-x` 拒)、不可达降级(无应答 → list Err、根列表不含 bridge 根)、visible 隐藏根不出列表但 check 放行
- [ ] **Step 2: 确认失败** Run: `cargo test -p mtdss-daemon serve` Expected: FAIL
- [ ] **Step 3: 实现 ServeBridge**(photo.rs 逐段对照:登记表/广播/超时/缓存不设——列表每次实问);ctrl List/Stat 命中 vpath → 桥;根列表 = visible bridge 根合成目录条目(children 惰性 None);jobs 收集 walk 用桥 list、文件源用 export 临时件;delete 路由(整树语义 = App 侧 SAF delete 递归,一次调用)
- [ ] **Step 4: 确认过** Run: `cargo test -p mtdss-daemon` Expected: PASS
- [ ] **Step 5: e2e**:fake 应答器(沿 FakeFdServer/photo e2e 先例):注册 bridge 根 → 对端 List 见根 → 下钻 list → download 到本地 md5 对 → delete 后 list 缺行;桥死 → 根消失 + 浏览报错
- [ ] **Step 6: golden**:serve 事件 JSON 三端增量(golden REGEN;Kotlin/Swift 契约文件同步常量)
- [ ] **Step 7: Commit** `feat(daemon): 服务桥——对端浏览/下载/删除安卓 SAF 内容(fs.serve_* + 0x08/09/0A;photo 模式泛化)`

### Task 3: 照片桥 Android 开闸

**Files:**
- Modify: `crates/daemon/src/main.rs:84`(photo_serve:`cfg!(any(target_os = "android", target_os = "ios"))`)
- Test: 既有 photo 测试零改动即过(门控面既有);golden 零增量(契约 iOS 已铺)

**Interfaces:**
- Produces:Android 引擎进程持 PhotoBridge → 广播 `fs.photo_list`/`fs.photo_export`(0x06/0x07);App 应答器归 T14

- [ ] **Step 1: 改门控 + 全量** Run: `cargo test -p mtdss-daemon` Expected: PASS(photo 矩阵在 cfg 门内既有)
- [ ] **Step 2: WSL 复跑**(android 目标不编本地;linux 全量证绿)Run: WSL cargo t -p mtdss-daemon Expected: PASS
- [ ] **Step 3: Commit** `feat(daemon): 照片桥 Android 开闸(权限即数据面归 App 应答器)`

### Task 4: SynshuTheme 主题 token 层 + 组件库(安卓)

**Files:**
- Create: `android/app/src/main/java/com/mtdss/android/ui/theme/SynshuTheme.kt`(浅/深 ColorScheme + 语义色常量 + 字阶)
- Create: `android/app/src/main/java/com/mtdss/android/ui/common/SynshuRows.kt`(SynshuRow/ExpandChevron/FileTypeIcons/SelectionCircle)
- Create: `android/app/src/main/java/com/mtdss/android/ui/common/SynshuBars.kt`(CenterTopBar/SelectionTopBar/SelectionActionBar/CapsuleNavBar/SearchField)
- Test: `android/app/src/test/java/com/mtdss/android/ui/theme/SynshuThemeTest.kt`

**Interfaces:**
- Produces(后续所有 UI 任务消费):`SynshuColors`(object 常量:Accent=#007AFF 等 spec §2 全值,深色档 `SynshuColorsDark`)、`SynshuRow(entry,depth,expanded,selecting,selected,…)`(签名对齐 iOS BrowseEntryRow 参数面)、`CapsuleNavBar(tabs,active,onTab)`、subtitle 格式器 `entrySubtitle(mtimeMs,children,size):String`(「日期 · N 项」/「日期 · 大小」)

- [ ] **Step 1: 写失败测试**:token 逐值断言(#007AFF/#F2F2F7/#8E8AA0/#FF3B30 等)+ `entrySubtitle` 格式三分支(目录 N 项/文件大小/未知 children 退化纯日期)
- [ ] **Step 2: 确认失败** Run: `gradlew.bat :app:testDebugUnitTest --tests "*SynshuThemeTest*"` Expected: FAIL
- [ ] **Step 3: 实现**(Compose M3 `lightColorScheme/darkColorScheme` 全 token 覆写;组件按 spec §2.2 规格值;胶囊 blur `Modifier.blur` API31 门 + 退化不透明)
- [ ] **Step 4: 确认过** 同上 Expected: PASS;`gradlew.bat :app:compileDebugKotlin` 过
- [ ] **Step 5: Commit** `feat(android): Synshu 主题 token 层与组件库(spec §2 逐值)`

### Task 5: 四标签壳与导航重写

**Files:**
- Modify: `android/app/src/main/java/com/mtdss/android/ui/AppNav.kt`(重写为四标签壳;旧路由共存到 T16)
- Create: `android/app/src/main/java/com/mtdss/android/ui/tabs/AppTabView.kt`(枚举 AppTab{favorites,devices,transfers,profile} + 胶囊壳)
- Test: `android/app/src/test/java/com/mtdss/android/ui/tabs/AppTabTest.kt`(枚举序/标题词面)

**Interfaces:**
- Consumes: T4 CapsuleNavBar/SynshuTheme;既有 `SessionViewModel`(登录态路由: splash/login/四标签)、`ApprovalSheetHost`(原样挂入)
- Produces:四标签壳;bootError 红胶囊横幅 + 「重连中」蓝胶囊;选择模式藏栏回调面(各 tab 经 `LocalCapsuleHidden` 或壳级状态——实现者沿 Compose 惯例择一,壳消费面钉死:`AppTabView(graph, selectionHidden: Boolean)`)

- [ ] **Step 1: 写失败测试**:AppTab 四枚枚举序与标题(快捷目录/设备/传输列表/我的)与 iOS AppTab.swift 逐字一致
- [ ] **Step 2-4: 红→实现→绿**(登录态翻转到四标签;被踢回登录既有语义不变)
- [ ] **Step 5: Commit** `feat(android): 四标签壳+胶囊底栏(旧屏共存期)`

### Task 6: 设备 tab + 选设备弹层

**Files:**
- Rewrite: `android/app/src/main/java/com/mtdss/android/ui/devices/DevicesScreen.kt`
- Create: `android/app/src/main/java/com/mtdss/android/ui/common/DevicePickerSheet.kt`(ModalBottomSheet: 拖把手/标题「发送|转发 N 项到…」/滤己/转发滤源)
- Test: `android/app/src/test/java/com/mtdss/android/ui/devices/DevicesViewModelTest.kt`(扩:本机行在列禁进/离线灰显数据面)

**Interfaces:**
- Consumes: T4 组件;既有 `DevicesViewModel`(device_id/name/platform/online/created_at)
- Produces:`DevicePickerSheet(graph,title,excluding,onPick)`(T7/T10 消费)

- [ ] **Step 1: 写失败测试**:VM 行序(created_at 定序既有)、本机行副文「Android · 本机」不可进、离线行禁点
- [ ] **Step 2-4: 红→实现→绿**(行 = 图标+名+「平台 · 状态」+状态点;···=刷新;无左滑)
- [ ] **Step 5: Commit** `feat(android): 设备标签+选设备弹层(iOS 同形)`

### Task 7: 浏览模板重写(远端/本地/成员目录通用)

**Files:**
- Rewrite: `android/app/src/main/java/com/mtdss/android/ui/browse/BrowseScreen.kt` + `BrowseViewModel.kt`
- Test: `android/app/src/test/java/com/mtdss/android/ui/browse/BrowseViewModelTest.kt`(重写扩展)

**Interfaces:**
- Consumes: T4/T6;既有 `MtdssRepository.ctrlList/ctrlDelete…`
- Produces(iOS BrowseViewModel 语义镜像,JVM 可测):`visibleRows`(树展开序)/`expandedPaths`/`isSelecting`/`selection`/`enterSelection`/`confirmSelection()`(**确认=只退出无副作用**)/`toggleExpand(path)`/`sort`(名称/日期/大小/种类)/`searchText` 过滤/`showHidden` 过滤(dotfiles)/`truncated` 尾注;远端档动作集 `dirActions`(含「快捷」);删除确认文案(单目录「删除文件夹及内容?」)

- [ ] **Step 1: 写失败测试**:确认语义(confirm 后 selection 清空、无副作用标志)、展开树序(父子插入位)、搜索/排序/隐藏过滤矩阵、删除文案三分支
- [ ] **Step 2: 确认失败** Run: `gradlew.bat :app:testDebugUnitTest --tests "*BrowseViewModelTest*"` Expected: FAIL
- [ ] **Step 3: 实现视模型;UI 按 §2.2 规格**(顶栏双箭头+绝对居中标题中截断+蓝三点;搜索框;行/展开/选圈;左滑 M3 SwipeToDismiss 自绘皮肤无全滑;下拉刷新 PullRefresh;选择顶栏/底栏;长按 0.4s)
- [ ] **Step 4: 确认过** Expected: PASS
- [ ] **Step 5: Commit** `feat(android): 浏览模板重写(iOS §4.1 模板逐项)`

### Task 8: 传输列表 tab

**Files:**
- Rewrite: `android/app/src/main/java/com/mtdss/android/ui/tasks/TasksScreen.kt`;Modify `TasksReducer.kt`
- Test: `android/app/src/test/java/com/mtdss/android/ui/tasks/TasksReducerTest.kt`(扩展)

**Interfaces:**
- Consumes: 既有 TasksViewModel/事件面;T4 徽章组件
- Produces:`TaskBadge.color`: DOWNLOAD=#007AFF / UPLOAD=#34A853 / SCHEDULE=#8E8AA0(**换向,旧橙/旧蓝绿全删**);行状态格式器(进行中「速度 · %」/完成「今天 HH:mm · 跳过 N 项(同名已存在)」/失败红字 error 空兜底「已中断」);排序 = created_ms 降序(**最新在上**);分段 Filter 全部/接收/发送/调度;左滑按状态出集(取消/重新发起+清除/清除);「分享转交失败」行位(T15 填充)

- [ ] **Step 1: 写失败测试**:徽章色断言新值、最新在上排序、分段过滤四档、左滑出集按状态矩阵、跳过段文案
- [ ] **Step 2: 确认失败** Expected: FAIL
- [ ] **Step 3: 实现**;**grep 门禁**:`grep -rn "0xFF1E88E5\|0xFF2E7D32\|0xFFEF6C00" android/app/src/main` 必须零命中(旧色全退役)
- [ ] **Step 4: 确认过** Expected: PASS + grep 零命中
- [ ] **Step 5: Commit** `feat(android): 传输列表重写(徽章换向/分段含调度/最新在上)`

### Task 9: 快捷目录 store + watcher + SWR(安卓)

**Files:**
- Create: `android/app/src/main/java/com/mtdss/android/favorites/FavoritesStore.kt`
- Create: `android/app/src/main/java/com/mtdss/android/favorites/FavoritesTree.kt`(成员行树模型)
- Test: `android/app/src/test/java/com/mtdss/android/favorites/FavoritesStoreTest.kt` + `FavoritesTreeTest.kt`

**Interfaces:**
- Consumes: 既有 `kv.get/kv.put` IPC、`event.data_changed`
- Produces(iOS FavoritesStore/MixedViewModel 语义镜像):单键 `"favorites"` 整单 LWW、`v>1` 只读零回写、put 成功才落;`FavoritesTree.members(favorites,devices)`(「设备名 - 目录名」行 + 离线判定 devices join);SWR:缓存种子/重进瞬时/后台再校验/members 签名失效/`clearFavCache()`(登出+被踢);`devices_changed` 快照迟到补拉

- [ ] **Step 1: 写失败测试**(对齐 iOS FavoritesStoreTests 语义):LWW 整单、v>1 只读、data_changed{favorites} 重拉他 key 忽略、登出清缓存、树成员离线灰显判定
- [ ] **Step 2-4: 红→实现→绿**(JSON 形与 iOS/Windows 逐字节同构——golden 键序)
- [ ] **Step 5: Commit** `feat(android): 快捷目录 store/树模型/SWR(V3-3 语义镜像)`

### Task 10: 快捷目录 tab UI

**Files:**
- Create: `android/app/src/main/java/com/mtdss/android/ui/favorites/FavoritesScreen.kt` + `FavoritesViewModel.kt`
- Test: `android/app/src/test/java/com/mtdss/android/ui/favorites/FavoritesViewModelTest.kt`

**Interfaces:**
- Consumes: T7 浏览模板推入(.member 路由)、T9 store/tree、T6 picker、T13 门
- Produces:根页(收件箱置顶 DirNode + 快捷行成员树:展开/点按推入/离线禁点/子级含文件/左滑 [下载,转发,删除]/快捷行左滑 [管理,删除快捷]/··· 选择+新建);管理 sheet(改名/增删成员);新建 alert 空名禁提交;readOnly 守卫

- [ ] **Step 1: 写失败测试**:VM 面(收件箱置顶序/成员展开状态机/新建空名拒/删除快捷确认后 store.delete/树行隐藏过滤)
- [ ] **Step 2-4: 红→实现→绿**(复用 T7 行组件;文案逐字对齐 iOS)
- [ ] **Step 5: Commit** `feat(android): 快捷目录标签(收件箱置顶+成员行树)`

### Task 11: 我的 + 设置 + 登录

**Files:**
- Rewrite: `android/app/src/main/java/com/mtdss/android/ui/settings/SettingsScreen.kt`、`ui/login/LoginScreen.kt`;Create `ui/profile/ProfileScreen.kt`
- Test: `SettingsViewModelTest.kt`(扩展)、`LoginViewModelTest.kt`(设备名迁移面)

**Interfaces:**
- Consumes: 既有 AccountStore 等价物、踢选/审批组件(重皮不动逻辑)
- Produces:我的页(扫码占位→T12/齿轮/账号行/占位行/退出红字居中确认/版本沉底);设置分组卡(设备名下次登录生效/信任目录入口→既有 SAF 根管理/显示隐藏开关默认关/允许删除远端文件开关默认关/检查更新/关于);登录页(邮箱/验证码+发送/CTA/设备名注脚只读/高级折叠;踢选屏新皮)

- [ ] **Step 1: 写失败测试**:设置默认值(双开关 false)、设备名改后「下次登录生效」注记、登录页不再提交设备名字段(取主机名默认)
- [ ] **Step 2-4: 红→实现→绿**
- [ ] **Step 5: Commit** `feat(android): 我的/设置/登录重做(设备名迁设置;双门默认关)`

### Task 12: 扫码器

**Files:**
- Create: `android/app/src/main/java/com/mtdss/android/ui/profile/ScanSheet.kt`、`ui/profile/QrApprovalModel.kt`
- Modify: `android/app/build.gradle.kts`(CameraX + zxing-core 依赖;版本用稳定钉版)
- Test: `android/app/src/test/java/com/mtdss/android/ui/profile/QrApprovalModelTest.kt`

**Interfaces:**
- Consumes: 既有 `qr.claim/qr.approve` IPC(V3-1 已铺,daemon 透传)
- Produces:`QrApprovalModel`(stage: idle→claiming→confirming(candidate_name/platform)→result/failed;approve(allow));ScanSheet(CameraX 预览 + ZXing 解析;相机权限拒→入口置灰+指路设置;扫得 token 即建模型)

- [ ] **Step 1: 写失败测试**:态机迁移矩阵(claim 成功→confirming;同意/拒绝→result;claim 失败→failed 文案;未登录置灰)
- [ ] **Step 2-4: 红→实现→绿**(候选确认弹层对齐 iOS 词面「扫码登录」「同意登录/拒绝/取消」)
- [ ] **Step 5: Commit** `feat(android): 扫码登录闭环(V3-1 顺延点亮)`

### Task 13: 显示隐藏 + 删除门消费接线

**Files:**
- Modify: `BrowseViewModel.kt`(T7 已留 showHidden 参数位)、`FavoritesViewModel.kt`、设置 VM
- Test: 对应 VM tests 扩展

**Interfaces:**
- Consumes: T11 设置开关持久(DataStore/SharedPreferences 沿既有)
- Produces:门控单源 `BrowseGates(showHidden, allowRemoteDelete)`(远端行删除/选择底栏删除/树行远端删除三处消费;本地删除不设门)

- [ ] **Step 1: 写失败测试**:门关时远端删除动作不出集、门开时在;showHidden=false 时 dotfile 行不渲染(数据在列表仅滤显示)
- [ ] **Step 2-4: 红→实现→绿**
- [ ] **Step 5: Commit** `feat(android): 双门消费(显示隐藏/允许删除远端文件)`

### Task 14: roots.set 同步 + SAF/照片应答器(安卓接线)

**Files:**
- Create: `android/app/src/main/java/com/mtdss/android/serve/ServeResponder.kt`(fs.serve_list/export/delete → SAF)
- Create: `android/app/src/main/java/com/mtdss/android/serve/PhotoResponder.kt`(fs.photo_list/export → MediaStore)
- Modify: `send/RootsStore.kt`(变更 → roots.set)、`MtdssEngine.kt`/`AppGraph.kt`(启动同步)、`AndroidManifest.xml`(权限声明)
- Test: `serve/ServeResponderTest.kt`、`serve/PhotoResponderTest.kt`(FakeSafFace 模式)

**Interfaces:**
- Consumes: T1 `roots.set`、T2 `fs.serve_*`(0x08/09/0A)、T3 `fs.photo_*`(0x06/07);既有 SafFace/FdPassChannel
- Produces:`rootsSpecs(inbox,authorizedRoots): List<RootSpec>`(收件箱 "/收件箱" + 授权根名;source=bridge,visible=true);SAF list/export(写 `{home}/serve-tmp/`)/delete(递归);照片权限门(未授权不应答 → 引擎侧根隐藏);运行时权限请求时机 = 设置页开启照片共享/首次被浏览前不发弹窗(权限即数据面:未授权=根不在)

- [ ] **Step 1: 写失败测试**:rootsSpecs 形(收件箱恒在/授权根映射)、SAF list 假面矩阵(docId→vpath 子路径)、export 写临时件路径与 unlink 语义、delete 递归、照片未授权 = responder 不注册
- [ ] **Step 2-4: 红→实现→绿**;权限矩阵 manifest(33+ READ_MEDIA_*/29-32 READ_EXTERNAL_STORAGE)
- [ ] **Step 5: 真机冒烟钩子**(adb 拉起对端浏览路径,smoke 项预告)
- [ ] **Step 6: Commit** `feat(android): SAF/照片应答器+roots.set 同步(服务桥安卓正身)`

### Task 15: 分享扩展暂存模型

**Files:**
- Rewrite: `android/app/src/main/java/com/mtdss/android/send/ShareActivity.kt`
- Create: `share/ShareQueue.kt`(队列 json + 暂存拷贝 + 尺寸守卫)、`share/ShareConsumer.kt`(主 App 消费)
- Modify: `TasksScreen`(失败行渲染位 T8 已留)
- Test: `share/ShareQueueTest.kt`

**Interfaces:**
- Consumes: 0x01 request_source(既有 PickerBridge,暂存件作源——**不进授权集**)
- Produces:入队两步(begin/copy/finish 原子);失败行 `ShareFailureRow{id,name,targetDeviceId}`;重试 = 重选设备重发;主 App 前台/登录消费触发

- [ ] **Step 1: 写失败测试**:拷贝中断(半件)不 finish、盘损截短被尺寸守卫拒、重试换目标设备、消费幂等(登录前不动队列)
- [ ] **Step 2-4: 红→实现→绿**(ShareActivity 新皮 = A 配色设备列表)
- [ ] **Step 5: Commit** `feat(android): 分享暂存转交可重试(对齐 iOS 方案A)`

### Task 16: 旧屏退役 + 契约对齐 + 全量绿

**Files:**
- Delete/吸收: `ui/local/LocalFilesScreen.kt`(并入浏览模板)、旧 `DevicesScreen/TasksScreen` 残件
- Modify: `ipc/Contract.kt`(roots.set + fs.serve_* 常量对齐 golden)
- Test: 全量四套件

**Interfaces:**
- Consumes: 全部前序
- Produces:旧路由全撤(本机浏览 = 快捷目录收件箱;任务 = 传输 tab);ContractGoldenTest 与 golden 零漂移

- [ ] **Step 1: 退役清单核对**(grep 旧类引用零命中)
- [ ] **Step 2: 全量**:cargo(Win 全量 + WSL 全量)/ gradlew / ui npm test+tsc+build / Mac swift 回归绿
- [ ] **Step 3: Commit** `refactor(android): 旧屏退役+契约对齐(V3-4 UI 收口)`

### Task 17: Windows 设置面 + 四项 + 顺手票

**Files:**
- Create: `ui/src/settings/SettingsDialog.tsx`(+ Tauri 命令桥 `ui/src-tauri/src/cmd_settings.rs`)
- Modify: `crates/daemon/src/settings.rs`(schema:allow_remote_delete/show_hidden/trust_mode/trusted_roots/device_name)
- Modify: `crates/daemon/src/main.rs`(Windows bin 启动读信任 conf → DaemonOpts;缺省 None 直通)
- Modify: `ui/src/App.tsx`(登录高级选项去设备名;远端删除入口受门;dotfile 过滤;truncated 浮面)
- Test: `ui/src/settings/__tests__/*.test.ts` + daemon settings tests

**Interfaces:**
- Consumes: T1 `roots.set`(开关切换热生效经 IPC);T2 无涉(Windows 全 fs source)
- Produces:设置对话框(工具栏齿轮;四项 + 「限制本机被浏览范围」+ 目录选择器);设置持久 daemon settings.json(引擎强制项)+ UI 存储(显示项);文案中文

- [ ] **Step 1: 写失败测试**(vitest):门默认关、信任模式开 → roots.set 调用载荷(收件箱+所选目录)、显示隐藏过滤、设备名迁移后登录表单无该字段
- [ ] **Step 2: 确认失败** Run: `npm test` Expected: FAIL
- [ ] **Step 3: 实现**(daemon settings tests:新字段缺省 = 旧行为;main.rs conf 读取缺省直通)
- [ ] **Step 4: 确认过**:npm test + cargo test -p mtdss-daemon settings PASS
- [ ] **Step 5: Commit** `feat(win): 设置面+删除门/信任模式/显示隐藏/设备名迁移+truncated 浮面`

### Task 18: 冒烟清单 + 文档

**Files:**
- Create: `docs/smoke-v3p4.md`
- Modify: `docs/dev-*.md` 相关面

**Interfaces:** 无代码

- [ ] **Step 1: 写清单**:安卓全屏走查(§3 逐屏)/ 八项逐项 / 权限矩阵(MIX 3 = 29 档)/ Windows 四项 / 分享重试 / 扫码闭环(MIX 3 ↔ PC)/ 服务桥三动作(md5 对拍)
- [ ] **Step 2: Commit** `docs: smoke-v3p4 冒烟清单`

---

## 执行注意

- **顺序**:T1→T2→T3(引擎)与 T4→T5(UI 基座)可并行开局;T6-T8 依赖 T5;T9/T10 依赖 T7+T2;T14 依赖 T2/T3;T17 仅依赖 T1。终审后按惯例:实机包装 + 版本 bump(四处版本文件+lock×2+golden REGEN+Kotlin 断言+versionCode,1.3.0 线)+ TestFlight 不涉(iOS 零改)。
- **金科玉律**:每任务双闸(实现+独立评审);golden 改动必三端 grep 消费面;cargo 绿判一律落盘+显式回显退出码(管道判绿=假绿教训)。
- **已知坑**:MIUI SAF 语义(保留点前缀/MIME 改写/createDirectory 拒已存在名);WSL 测试须 `MSYS_NO_PATHCONV=1`;本地 shell 残留 env(MTDSS_UPDATE_URL 等)按 runbook 清;gradlew 在仓库 android/ 目录下跑。
