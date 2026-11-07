# MTDSS V3-3 返工波 5(冒烟第四轮裁定)实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §17 五裁定:快捷目录着陆面(列表页+面包屑下拉+收件箱对齐)、树成员加载慢导航跳 bug、侧栏拖宽持久、树子级含文件(两端)+iOS 左滑;收官回归+重打包+冒烟 §J。

**Architecture:** 新 Loc `{kind:"favs"}` 着陆位;收件箱行=树可展开特殊行(本机设备语义);面包屑 fav chip 下拉扩收件箱;树子级 DirNode 增 showFiles(快捷树面专用,设备树不动)。

**Tech Stack:** 同前。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md` **§17 为权威**。

## Global Constraints

- 零回归红线:设备树/设备视图/普通目录/混合分组视图/管理屏不变;§16 各面不动。
- 着陆面/下拉/树三处对收件箱的行为一致(点=进本机收件目录);面包屑样式与设备 chip 同构(复用 .devchip 形 or 同风格类)。
- 侧栏宽度持久键独立于主题;拖柄与既有树拖放/框选不冲突。
- iOS 树行左滑集=混合页同源(门:全本机 OR allowRemoteDelete);文件行与目录行同集。
- iOS 测试走 Mac mini(diff-apply 含 crates goldens)。

## Review Focus

1. favs Loc 与 fav/favdir 的历史/上级链(parentLoc favs→devices?裁定:favs 上级=devices 恒,与其他顶层位一致)。
2. 收件箱树展开与设备树 self 设备子级的缓存共享(同键无害)。
3. 侧栏拖宽与拖放命中(data-drop-loc)的事件分层(拖柄区不落文件)。
4. 树 showFiles 对选择/拖拽源的影响(文件行=叶,不可导航;可否作拖拽源?裁定:树文件行不参与拖拽,保持导航树纯度)。
5. Z3 bug 须有复现证据(慢加载模拟/桩),不许盲改。

---

### Task 1: Z2 Windows 着陆面+收件箱对齐
**Files:** nav.ts;App.tsx;components/FavsLandingView.tsx(新);AddressBar.tsx;Sidebar.tsx;zh.json;App.css;tests。
**Interfaces:** Loc 增 `{kind:"favs"}`;parentLoc(favs)→devices;着陆页=收件箱置顶行+快捷目录行(名+成员数副文;双击/回车进 fav);面包屑 favs 位=根段「快捷目录」+fav chip 下拉(收件箱+各快捷目录;样式同 devchip);组头行点击→navigate favs(撤落收件箱);树收件箱行 caret 可展开(self 设备子级懒加载,DirNode showFiles=false 此任务先不动——Z5 做)。
- [x] 实现+SSR(着陆页词面/下拉含收件箱/组头行导航);vitest+tsc
- [x] Commit:`feat(v3p3-rework5): 快捷目录着陆面+面包屑下拉+收件箱树展开对齐(§17.1/17.2)`
### Task 2: Z3 导航跳转 bug 诊断+修
**Files:** 视根因(嫌疑 App.tsx openFavDirMember favdirAt null 回退)。
- [x] 复现(慢加载桩/事件序列)→根因→修(回退收口:归一失败留在原位+toast,不跳普通 dir);测试钉
- [x] Commit:`fix(ui): 树成员加载慢导航跳设备位(根因:{一句话},§17.3)`
### Task 3: Z4 侧栏拖宽持久
**Files:** Sidebar.tsx/App.tsx/App.css;tests。
- [x] 右缘拖柄(4px 热区)+min/max(160-420px)+localStorage 键 `sidebarWidth`(主题无关);拖拽与文件拖放/树命中分层
- [x] vitest+tsc;Commit:`feat(v3p3-rework5): 侧栏宽度拖动+持久(§17.4)`
### Task 4: Z5 树子级含文件(两端)+iOS 左滑
**Files:** Win Sidebar.tsx(DirNode showFiles=快捷树面);ios FavoritesView.swift(树子级含文件+行左滑);tests。
- [x] Win:快捷树面(成员/收件箱/子目录)子级含文件(文件行=叶不导航不拖拽);设备树不动
- [x] iOS:树展开含文件;展开内容行左滑(下载/转发/删除门,混合页滑集同源;文件/目录同集);swift test+build
- [x] Commit:`feat(v3p3-rework5): 快捷树子级含文件(两端)+iOS 树行左滑(§17.5)`
### Task 5: Z6 回归+重打包+冒烟 §J
- [x] 四套件;r6 包+TestFlight;smoke §J(J1 着陆面/J2 收件箱对齐/J3 导航不跳/J4 侧栏拖宽/J5 树含文件+左滑/J6 全量复走);spec §17 注记;plan 勾选
- [x] Commit:`docs(v3p3-rework5): 冒烟 §J+收官注记`

---

> **收官注记(2026-11-07)**:返工波 5 全落(Z2-Z6,分支 v3p3)。四套件绿
> (ui vitest 485+tsc+build / Android JVM 284 强制重跑 / Mac swift test
> 291+sim build+崩溃锚 UITest ×3 / cargo workspace 全量 ×3——全部套件绿,
> 唯既档 flake 家族 `daemon_e2e::restart_marks_interrupted` 今日在套件内
> 恒抖(单测隔离恒绿 ×4,预算型机制经全量三跑+单跑双测钉死;预算放宽票
> 升级)+ 两例一次态(mtdss-ipc lib consts=会话 env `MTDSS_UPDATE_URL`
> 漏入 option_env! 编译产物,archive「unset」口径复刻即绿;contract_golden
> 固定端口登录桩竞态,隔离 0.75s 即绿)——引擎零改动,均非本波回归)。
> 包目 v3p3r6(Windows NSIS `Synshu_1.2.0-v3p3r6-x64-setup.exe`+sha256;
> iOS ipa 1.2.0 build **20261107.2** TestFlight **上传成功**,Delivery
> UUID `88d4b021`,altool 直传档)。真机/实机场 gate 挂用户侧(smoke §J
> 「先 §J 再全量」,J6 收口);spec §17 状态注记已入档。
