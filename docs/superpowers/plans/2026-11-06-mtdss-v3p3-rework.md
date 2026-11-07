# MTDSS V3-3 返工波(冒烟第一轮裁定)实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §13 九条返工裁定:树形侧栏两端、新建下拉、收件箱目录创建、64 色板(用户钦定)按加入顺序分配、分组节收起、面包屑快捷命名空间、iOS 崩溃修复、iOS 树形展开、全端改名「快捷目录」;收官重打包+冒烟第二轮。

**Architecture:** 承接 v3p3 既有面(favorites 模型/store/混合视图/管理屏/iOS 三屏);新增契约=devices 快照携 `created_at`(加入顺序数据源);Loc 增 `favdir`(快捷命名空间钻取);色板常量三端 verbatim 内嵌。

**Tech Stack:** 同 V3-3 主波(Rust/Tauri/React+TS+vitest/SwiftUI+MTDSSKit/Mac mini SSH 测试)。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md` **§13 返工裁定为权威**(与 §5/§4 冲突处以 §13 为准)。

## Global Constraints

- 色板 64 值以 spec §13.4 json 块 **verbatim** 内嵌(逐字节,不得重排/重新生成);分配=devices 按 `created_at` 升序位次,行主序取色(第 n 台=palette[n-1],n>64 回绕 palette[(n-1)%64])。
- 「常用目录」UI 文案全端改「快捷目录」;技术键 `favorites`、kv、目录名 `MTDSS` 不动。
- 桌面零回归红线延续:普通目录(设备树/浏览/拖放/粘贴)行为除明示项外不变;iOS 既有浏览/选择/滑删面不变。
- 每任务独立评审;golden REGEN 纪律;iOS 测试走 Mac mini(diff-apply 集**必须含 crates/ipc/contract golden**);中文注释房风。

## Review Focus

1. created_at 缺省/为零的旧服务端数据:加入顺序排序须稳定(created_at 同值按 device_id 决胜,三端一致)。
2. favdir 面包屑与历史/上一级:rel="" 顶级回混合视图;设备隐藏不破「复制路径」等既有功能(在 favdir 位禁用或按原路径?——裁定:favdir 位复制路径给成员原始绝对路径)。
3. 64 色回绕(>64 台)与设备被移除后位次移位:颜色随位次漂移,接受(spec §13.4 语义)。
4. iOS 崩溃修复须有复现证据(模拟器/日志)或双候选根修,不许盲改。
5. 改名不得触及 kv 键/文件路径/契约字符串(仅人读文案)。

---

### Task 1: 契约——devices 快照携 created_at

**Files:** server/signaling/src/api.rs(devices REST);crates/daemon/src/(devices op 组装处);crates/ipc/contract/devices.resp.json(REGEN);ui/src/lib/derived.ts(DeviceInfo);ios/package/Sources/MTDSSKit/(Contract DirectoryDevice);android 对应模型/契约测试。
**Interfaces:** Produces: `devices` 响应行增 `created_at: i64`(Unix 秒,服务端既有列);TS `DeviceInfo.created_at?: number`(旧 daemon 缺省 undefined→排序按 device_id);Swift `DirectoryDevice.createdAt: Int64?`。服务端表已有 created_at(核查列名)。
- [x] 服务端 devices REST 增字段(默认序=created_at 升序返回,排序即加入顺序)→ Rust 单测
- [x] daemon devices op 透传;golden REGEN(devices.resp.json);Kotlin/Swift 契约测试同步
- [x] TS/Swift 类型增字段(缺省容忍)+ 测试;cargo test / swift test / gradlew 全绿
- [x] Commit:`feat(contract): devices 快照携 created_at(终端加入顺序数据源,V3-3 返工 R1)`

### Task 2: daemon 启动建收件箱目录

**Files:** crates/daemon/src/main.rs(resolve 后);tests。
- [x] 失败测试:resolve_recv_dir 指向不存在目录 → daemon 启动后目录存在(集成测试形态沿既有 daemon_basic)
- [x] 实现:main.rs resolve 后 `let _ = std::fs::create_dir_all(&recv_dir);`(失败 tracing::warn,不阻启)
- [x] cargo test -p mtdss-daemon 绿;Commit:`fix(daemon): 启动即建收件箱目录(文档\MTDSS 缺失→收件箱打不开,V3-3 返工 R2)`

### Task 3: 64 色板 + 加入顺序分配(三端)

**Files:** ui/src/lib/favorites.ts(色板常量+deviceColorMap 重写);ui/src/lib/favorites.test.ts;ios/.../FavoritesStore.swift(Palette 重写);ios tests;Sidebar.tsx(撤成员色点串);FavView/FavoritesMixedView(色点取值面核对)。
**Interfaces:** `export const DEVICE_COLOR_HEX: readonly string[]`(spec §13.4 verbatim 64 值);`deviceColorMap(devices: {device_id; created_at?: number}[]): Map<string,string>`(created_at 升序、缺省排尾按 device_id、位次 mod 64)。Swift `FavoritesPalette.hexes`(去 `#` 前缀同序)+ `colorMap(devices:)` 同语义。
- [x] TS:重写+测试(64 值逐字节断言、排序三态、回绕);Sidebar 撤串;vitest+tsc
- [x] Swift:镜像+测试(64 值断言);Mac swift test
- [x] Commit:`feat(v3p3-rework): 64 色板钦定版落地,按终端加入顺序分配(§13.4)`

### Task 4: Windows 侧栏树形重构

**Files:** ui/src/components/Sidebar.tsx;App.tsx(接线);zh.json。
**Interfaces:** Sidebar 常用区改为:`快捷目录` 组节点(可展开,caret 同设备组;默认展开)→ 收件箱行(置顶)→ 快捷目录条目(可展开:成员目录节点=DirNode 语义复刻,depth+1,懒加载子目录,点=进 favdir)→ 成员子目录逐级。点条目本身=onOpenFav(混合视图)。右键管理保留;`+` 撤(R5 接新建下拉前先撤)。
- [x] 重构+SSR 冒烟测试(组可展开/收件箱置顶/成员节点懒加载语义纯函数面)
- [x] vitest+tsc;Commit:`feat(v3p3-rework): 侧栏快捷目录树形化(成员目录=设备同级节点,§13.1)`

### Task 5: Windows 新建下拉

**Files:** ui/src/components/Toolbar.tsx(「新建」钮激活);App.tsx;zh.json。
**Interfaces:** 新建钮 → 下拉(首项「快捷目录」;后续可扩展)→ onNewFav(既有 FavNameDialog 流)。侧栏 `+` 已撤。
- [x] 实现+Toolbar SSR 测试(菜单项词面);vitest+tsc
- [x] Commit:`feat(v3p3-rework): 新建按钮激活,下拉新建快捷目录(§13.2)`

### Task 6: Windows 面包屑快捷命名空间(favdir Loc)

**Files:** ui/src/lib/nav.ts;App.tsx;components/AddressBar.tsx;FavView(钻取改 favdir)。
**Interfaces:** `Loc` 增 `{kind:"favdir"; favId; device; base; rel}`(base=成员路径,rel=相对子路径,""=成员根);parentLoc:rel>""→去一段,rel=""→fav 混合视图;AddressBar favdir 渲染:`快捷目录`(根 crumb,点击→fav 视图?裁定:点击回该 fav 首层;无独立「快捷目录层」)> fav 名(**下拉切换其他快捷目录**)> rel 段;设备 chip/复制路径(给 base\rel 原始绝对路径)/编辑态在 favdir 位的语义:编辑态禁用(裁定,同 fav 位)。钻取(FavView 双击目录/侧栏成员节点点击)=navigate favdir;favdir 位主区=普通目录三视图(行=path 粒度,现有 dirProps 复用,数据源=该 device 的 list(base+rel))。
- [x] nav.ts favdir+测试;AddressBar 渲染+下拉;App 接线(favdir 拉取复用 dir 位 effect,dirKey 键含 fav 命名空间判断)
- [x] vitest+tsc;Commit:`feat(v3p3-rework): 面包屑快捷命名空间 favdir(隐藏设备与原始路径,§13.6)`

### Task 7: Windows 分组节展开/收起

**Files:** ui/src/components/FavView.tsx;App.css;zh.json。
- [x] 节头 chevron+展开态(默认全展;localStorage 记忆?裁定:会话内 state,不持久);SSR 测试;vitest+tsc
- [x] Commit:`feat(v3p3-rework): 分组节可展开收起(§13.5)`

### Task 8: 全端改名快捷目录

**Files:** ui/src/i18n/zh.json 及字面量;ios/app 文案;docs/smoke-v3p3.md 术语;spec 已改。
- [x] Windows/iOS 全量替换(仅人读文案;grep 常用目录 零残留于 UI 面)+ vitest/tsc/构建
- [x] Commit:`feat(v3p3-rework): 常用目录改名快捷目录(§13.9)`

### Task 9: iOS 崩溃复现取证+修复

**Files:** 视根因而定(候选:MixedViewModel.swift:136 Dictionary(uniqueKeysWithValues)、FavoritesView/FavoritesMixedView 生命周期)。
- [x] Mac 模拟器复现(离线成员+连点;TestFlight 亦可取 .ips);拿崩溃栈定位
- [x] 修复+防御(重复键折叠等)+ 回归测试;swift test + 构建
- [x] Commit:`fix(ios): 快捷目录打开崩溃(根因:{复现证据},§13.7)`

### Task 10: iOS 树形展开 + 对齐(色板/加入顺序/改名)

**Files:** ios/app/MTDSS/Browse/FavoritesView.swift;FavoritesMixedView(色点对齐);FavoriteManageSheet/Picker(文案)。
**Interfaces:** 快捷目录行=行内展开(成员目录行 depth+1 → 子目录逐级懒加载,沿 BrowseViewModel 展开机制或局部 VM);点子目录=推入浏览页(标题=目录名,设备隐含);色点=新色板/加入顺序;文案=快捷目录。
- [x] 实现+swift test+构建;Commit:`feat(ios): 快捷目录树形展开+64 色/加入顺序/改名对齐(§13.8)`

### Task 11: 重打包 + 冒烟第二轮手册增补

**Files:** artifacts/synshu-v3p3-smoke/(新包);docs/smoke-v3p3.md(增 §G 返工验证项:树形/新建下拉/收件箱/64 色/分组收起/面包屑/iOS 树形+崩溃回归);spec §13 状态注记。
- [x] 全量回归四套件;Windows NSIS 重打;Mac device-sign+TestFlight(新 build 号)
- [x] Commit:`docs(v3p3-rework): 冒烟第二轮手册增补+收官注记`

---

> **收官注记(2026-11-06,R11)**:R1-R11 全落——§13 九条裁定全实施(树形两端/新建下拉/收件箱自动建/64 色钦定版/分组节收起/面包屑 favdir 命名空间/iOS 崩溃根修 NavPathGate/iOS 树形/全端改名快捷目录),R3 遗留①色板全量断言与 R8 遗留②冒烟 A1 过程段一并收口。收官重打包:Windows NSIS `artifacts/synshu-v3p3-smoke/Synshu_1.2.0-v3p3r2-x64-setup.exe`(不入库)+ TestFlight build 20261106.2。全量回归四套件随本提交执行(结果见 `.superpowers/sdd/2026-11-06-mtdss-v3p3-rework/task-11-report.md`);冒烟第二轮手册 §G 编制毕(docs/smoke-v3p3.md),真机 gate 挂用户侧。
