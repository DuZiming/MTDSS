# MTDSS V3-3 返工波 2(冒烟第二轮裁定)实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §14 四裁定:两端树形=成员内容合并、跨成员下载 bug 修复、面包屑命名空间修正、列头固定/悬停/分组收敛;iOS 崩溃二轮根修;收官回归+重打包+冒烟 §H。

**Architecture:** 承接 v3p3 既有面;S2/S6 重写两端树展开(成员节点→内容并集,复用混合纯模型 offlineMembers/mixedRows 的树投影变体);S3 改 AddressBar 根段命名空间感知;S4 改 FileDetails(粘性列头+悬停域)+FavView 分组(单列头)。

**Tech Stack:** 同前(Rust/Tauri/React+TS+vitest/SwiftUI+MTDSSKit/Mac mini SSH)。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md` **§14 为权威**(与 §13 冲突处以 §14 为准)。

## Global Constraints

- 树形内容合并的排序=当前排序菜单语义(文件夹恒前);行键=复合键 device\0path;离线成员条目缺席(不可展开提示沿注记行)。
- 零回归红线:设备树/普通目录浏览/混合主视图行为不变(混合视图仍是首层导航面,树是其镜像入口)。
- 列头三项对**所有 dir/favdir 位**生效(不只快捷目录);分组节标签行样式沿 .fav-group-head。
- iOS 测试走 Mac mini(diff-apply 集含 crates golden);中文注释房风。

## Review Focus

1. 树投影的渐进/世代语义(成员应答迟到与树节点缓存交互;换快捷/重展开弃旧)。
2. 双组下载修复须有失败复现证据(单测/桩)而非仅改代码。
3. 面包屑根段点击语义(快捷目录根段点击→设备视图)与编辑态/复制路径在各新位不破。
4. 粘性列头与横向滚动/框选/拖拽 ghost 的 z 序不打架。
5. iOS 崩溃二轮须复现取证(同 §13.7 纪律)。

---

### Task 1: S3 Windows 面包屑命名空间修正
**Files:** ui/src/components/AddressBar.tsx;App.tsx;zh.json;AddressBar.test.tsx。
**Interfaces:** AddressBar 增可选 prop `favRoot?: { inboxPath?: string | null }`——简化裁定:组件内据 current+既有 props 自推:根段 label = fav/favdir 位→t("sidebar.favorites")(「快捷目录」,点击→devices);dir 位且 current.device===selfDeviceId 且 path 以 recv_dir 起段→根段「快捷目录」+首段「收件箱」(recv_dir 自身段),下级段照常(App 传 recv_dir 比较基准——经既有 favCrumbs 通道扩一个 inboxBase?: string);其余位维持「Synshu 设备」。
- [x] 根段/收件箱段实现+SSR 测试(fav 位含快捷目录+fav 名;favdir 含根段非 Synshu 设备;inbox dir 位含 收件箱 段;设备位零回归)
- [x] vitest+tsc;Commit:`feat(v3p3-rework2): 面包屑命名空间修正(快捷目录/收件箱根段,§14.3)`

### Task 2: S5 跨成员多选下载只下一个(bug 诊断+修复)
**Files:** 视根因而定(嫌疑:App.tsx downloadMixed/runGroupsWithConflictPause 消费、mixedview.groupRowsByDevice、FavView onRowsChange 快照)。
- [x] 复现:纯函数层单测构造双组选择(若纯层正确)→ 桩层/e2e 复现运行时丢组;拿失败证据
- [x] 修复+回归测试(双组双发起断言);vitest+tsc
- [x] Commit:`fix(ui): 跨成员多选下载丢组(根因:{一句话},§14.2)`

### Task 3: S2 Windows 树形=成员内容合并
**Files:** ui/src/components/Sidebar.tsx(树);ui/src/lib/mixedview.ts 或新 lib/favtree.ts(树投影纯函数);App.tsx(接线);Sidebar.test.tsx。
**Interfaces:** 新纯函数 `favTreeChildren(members, deviceOnline, caches): TreeNode[]` 形态裁定:首层=各在线成员 mixedRows 并集排序投影(文件+目录);目录节点懒加载子级(daemonList 该设备路径,复用既有 cache);文件节点=叶(不响应)。Sidebar FavNode 展开渲染该投影(替换 R4 成员 DirNode 形态);点目录节点=navigate favdir(经 favdirAt 归一);离线成员缺席+底部注记行(N 成员离线)。
- [x] 纯函数+vitest(并集排序/复合键/世代弃旧/离线缺席)
- [x] Sidebar 重构+SSR;vitest+tsc
- [x] Commit:`feat(v3p3-rework2): 侧栏树形=成员内容合并(§14.1)`

### Task 4: S4 Windows 列头收敛(粘性/悬停/分组单列头)
**Files:** ui/src/components/FileDetails.tsx;FavView.tsx;App.css。
**Interfaces:** FileDetails 增可选 `headerSticky?: boolean`(缺省现行为?裁定:直接全局改=粘性,dir/favdir 位一致;表头 hover 高亮仅 header 自身区域);FavView 分组模式:列头抽出到分组容器顶部渲染一次(节内 View 传 headerless 变体——FileDetails 增可选 `headerless?: boolean`),节标签行保留。
- [x] 粘性 CSS(position:sticky top:0 背景 token)+悬停域修正+分组单列头;SSR 断言(分组模式列头计数=1)
- [x] vitest+tsc;Commit:`feat(v3p3-rework2): 列头固定/悬停高亮域/分组单列头(§14.4)`

### Task 5: S6 iOS 树形=成员内容合并+文件可见
**Files:** ios/app/MTDSS/Browse/FavoritesView.swift(树展开重写);Kit 若需树投影纯函数入 MixedViewModel 或新类型;tests。
**Interfaces:** 镜像 S2:快捷行展开=成员内容并集(文件+目录,MixedViewModel.reload 的 states 或独立轻量拉取投影);目录行再展(该设备子级);点目录→推 favdir 语义浏览页(既有 member 路由);文件行可见不响应;离线成员缺席+注记。经 NavPathGate。
- [x] 实现+swift test+构建;Commit:`feat(ios): 快捷目录树形=成员内容合并+文件可见(§14.1/§14.5)`

### Task 6: S7 iOS 崩溃二轮复现+根修
**Files:** 视根因(新包 20261106.2,场景=新建快捷目录后点入)。
- [x] Mac 模拟器/UITest 复现(新建快捷→点入;空成员/离线成员两态);取证
- [x] 根修+回归锚;swift test+构建;Commit:`fix(ios): 新建快捷目录点入崩溃二轮(根因:{一句话},§14.6)`

### Task 7: S8 回归+重打包+冒烟 §H
- [x] 四套件全量;Windows r3 包+TestFlight 新 build;smoke §H(七项:H1 树内容合并/H2 双成员下载/H3 面包屑/H4 列头/H5 iOS 树/H6 iOS 崩溃回归/H7 全量 §A-§G 复走);spec §14 状态注记;plan 勾选
- [x] Commit:`docs(v3p3-rework2): 冒烟 §H+收官注记`

---

## 收官注记(2026-11-06,Task 7/S8 回填)

- **收官 commit**:ef1aff6..bcb4e3c(S5 修复轮起)→ 本注记 commit 止;波序
  S3→S5→S2→S4→S6→S7→S8 全落地,spec §14 状态注记在案。
- **四套件回归**(终 HEAD bcb4e3c):cargo 全 workspace 绿(cargo 首跑唯一红=
  `ctrl_e2e list_remote_roundtrip` 15s 超时,既档 flake 家族成员[v2p2 计划门禁
  注:打洞超时重跑均绿],隔离复跑 1.46s 绿);ui vitest 429/tsc/build 一次绿;
  gradlew :app:testDebugUnitTest 绿(首跑 gradle daemon TimeoutException 基建
  flake,复跑绿);Mac mini swift test 280/0 + ios/build.sh app BUILD-OK +
  UITest 锚 3 枚全绿(FavTapCrash×2/FavNewCrash×1,rig 沿 S7)。
- **包目 v3p3r3**:Windows `artifacts/synshu-v3p3-smoke/Synshu_1.2.0-v3p3r3-x64-setup.exe`
  (+sha256 旁档);iOS TestFlight build **20261106.3**。
- **冒烟 §H 编制入册**(docs/smoke-v3p3.md):H1-H8(八项:树合并两端/双目录
  下载/面包屑/列头/iOS 树/崩溃回归/全量复走/list 扇出观察项)+ §A-§G 词面同步
  (B1 面包屑两段式、C5 拖出两样都落随 S5);真机 gate 挂用户侧(H8 仅记录)。
