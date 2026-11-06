# MTDSS V3-3 返工波 3(冒烟第三轮裁定)实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §15 六裁定:混合视图无感化(缓存优先+动画插入)、组头行导航对齐、空目录传输修复、分组列宽钉死、fav 位搜索/复制/编辑三件、iOS 左滑/批量/搜索补面;收官回归+重打包+冒烟 §I。

**Architecture:** W1=TS favfetch 缓存层(SWR)+FavView 动画插入;Swift 侧 MixedViewModel 缓存+List 动画。W5=AddressBar 门控翻转+lib/address 快捷命名空间语法。其余独立。

**Tech Stack:** 同前。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md` **§15 为权威**。

## Global Constraints

- 缓存按 fav.id 键、登出/整单变更(members 变)失效;静默校验不得回退到空表(差异应用)。
- 零回归红线:设备树/普通目录/分组语义不变;首次进入仍渐进(不阻塞)。
- 空目录修复须含 e2e(两端各一枚空目录传输断言)。
- iOS 测试走 Mac mini(diff-apply 含 crates goldens)。

## Review Focus

1. 缓存与渐进的交互:缓存渲染中途后台校验应答到达——不得闪烁重排(差异最小应用或静默替换)。
2. FLIP 动画与拖拽/框选/虚拟滚动(如有)不打架。
3. 快捷命名空间地址解析与设备路径解析的歧义(设备名「快捷目录」冲突处理)。
4. 列宽钉死与列宽拖动记忆(colwidths 模块)共存。
5. 空目录传输:发送/转发/拖放/粘贴四入口一致。

---

### Task 1: W1 混合视图无感化(两端)
**Files:** ui/src/lib/favfetch.ts(缓存层);ui/src/components/FavView.tsx(动画);ios MixedViewModel.swift(缓存);FavoritesMixedView.swift(动画核实)。
- [ ] TS:fav 级 SWR 缓存(再进瞬时整表;校验差异静默应用);FavView 渐进插入动画(FLIP 或等价);vitest(缓存命中零拉取/失效面/差异应用)
- [ ] Swift:同象缓存+List 稳定 id 动画;swift test
- [ ] Commit:`feat(v3p3-rework3): 混合视图无感化——缓存优先渲染+动画插入(§15.1)`

### Task 2: W2 组头行导航对齐
**Files:** ui/src/components/Sidebar.tsx;App.tsx;Sidebar.test.tsx。
- [ ] 组头行点击=onOpenInbox(caret=toggle);SSR 断言更新;vitest+tsc
- [ ] Commit:`feat(v3p3-rework3): 快捷目录组头行导航对齐(落收件箱,§15.2)`

### Task 3: W3 空目录传输修复
**Files:** 视根因(crates/core 或 daemon 收集/校验面)。
- [ ] 复现(单测/e2e 空目录传输失败)→根因→修(空目录合法:对端建目录即成);e2e 断言
- [ ] Commit:`fix(engine): 空目录传输必败(根因:{一句话},§15.3)`

### Task 4: W4 分组列宽钉死
**Files:** ui/src/components/FileDetails.tsx;App.css。
- [ ] 列宽恒定(table-layout:fixed 或 colW 钉列);收起只藏行零位移;SSR/手测注记
- [ ] Commit:`feat(v3p3-rework3): 分组收起列宽钉死表头零位移(§15.4)`

### Task 5: W5 fav 位三件套
**Files:** ui/src/components/AddressBar.tsx;App.tsx;lib/address.ts;zh.json;tests。
- [ ] 搜索 fav 位启用(FavView 滤合并行;核实 favdir);复制路径 favdir 核实可用;编辑 favdir 位启用快捷命名空间语法(解析+回填);vitest+tsc
- [ ] Commit:`feat(v3p3-rework3): fav 位搜索/复制/编辑三件(快捷命名空间地址,§15.5)`

### Task 6: W6+W7 iOS 左滑/批量/搜索
**Files:** ios FavoritesMixedView.swift(+Kit 若需);tests。
- [ ] 行左滑(下载/转发/删除门);选择批量核实补齐;搜索框滤合并行;swift test+build
- [ ] Commit:`feat(ios): 混合页左滑/批量核实/搜索(§15.6)`

### Task 7: W8 回归+重打包+冒烟 §I
- [ ] 四套件;Windows r4 包+TestFlight 新 build;smoke §I(七项:I1 无感/I2 组行/I3 空目录/I4 列宽/I5 三件套/I6 iOS 补面/I7 全量复走);spec §15 注记;plan 勾选
- [ ] Commit:`docs(v3p3-rework3): 冒烟 §I+收官注记`
