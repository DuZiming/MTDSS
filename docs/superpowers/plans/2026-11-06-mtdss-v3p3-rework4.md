# MTDSS V3-3 返工波 4(展示方式重规划)实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 落地 spec §16:主区分类恒定(混合移除+色标移除+轻量缓存)、树=成员节点、组头行导航、列宽钉死、三件套(搜索/复制/编辑)、空目录传输修复、iOS 全套对齐(分类恒定/树成员行/左滑/批量/搜索);收官回归+重打包+冒烟 §I。

**Architecture:** 大量删除面(混合/色点/切换)+形态回归(树=§13.1 成员节点);缓存收窄为 SWR(节级);favdir 命名空间/拖放/动作对等/管理屏全部保留不动。

**Tech Stack:** 同前。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md` **§16 为权威**(§15.2-15.6 继续有效)。

## Global Constraints

- 删除即删除:混合投影(mixedRows 消费)、org 切换(Toolbar 段/@AppStorage/mtdss.fav.org)、色点(sourceColor/dotColor 消费)UI 面清零;库层纯函数与 64 色板/created_at 契约**休眠保留**(测试不动)。
- 零回归红线:设备树/普通目录/收件箱/管理屏/拖放三路/跨源分组动作(下载 local_copy 等)全部不变。
- 分类节=成员目录(同设备多成员=多节);节头「设备名 - 目录名」;离线成员缺席+注记行(原语义)。
- 缓存:fav 级 SWR——重进/子目录返回瞬时整表;后台静默校验差异应用;members 变更/登出失效。
- iOS 测试走 Mac mini(diff-apply 含 crates goldens)。

## Review Focus

1. 删除完整性:混合/色点残留 grep 零命中(UI 面);休眠库未被误删。
2. 分类恒定下的渐进:成员到达=整节追加不重排既有节;缓存种子与设备快照重判(离线翻桶)交互。
3. 树成员节点与 favdirAt 归一(点成员节点 rel="")、设备名-目录名标签、离线灰禁。
4. 三件套:快捷命名空间地址解析歧义(设备名「快捷目录」/路径分隔符);搜索跨节过滤+计数。
5. 空目录:发送/转发/拖放/粘贴四入口一致,e2e 钉死。

---

### Task 1: X2 Windows 主区分类恒定(删混合/色标+缓存)
**Files:** ui/src/components/FavView.tsx;Toolbar.tsx;App.tsx;favfetch.ts;App.css;tests。
- [ ] 删:org 切换(Toolbar 段+viewOrg 状态+@AppStorage)、mixedRows 消费、sourceColor/dotColor 消费;FavView 恒分组(节头=设备名 - 目录名)
- [ ] 增:fav 级 SWR 缓存(重进瞬时;校验差异静默应用;members 变/登出失效)
- [ ] vitest(缓存命中零重排/失效/SSR 节头词面)+tsc;Commit:`feat(v3p3-rework4): 主区分类恒定——混合与色标移除+SWR 缓存(§16.1/16.2/16.4)`
### Task 2: X3 Windows 树=成员节点+组头行导航
**Files:** ui/src/components/Sidebar.tsx;App.tsx;tests。
- [ ] 树:成员节点(设备名 - 目录名,§13.1 形态回归)替换内容并集;离线灰禁;点=favdir rel=""
- [ ] 组头行点击=导航(落收件箱);caret=展开
- [ ] vitest+tsc;Commit:`feat(v3p3-rework4): 树=成员节点(设备名-目录名)+组头行导航(§16.3/§15.2)`
### Task 3: X4 Windows 列宽钉死
**Files:** FileDetails.tsx;App.css。
- [ ] 分组收起/展开零列宽重协商;表头零位移;Commit:`feat(v3p3-rework4): 分组列宽钉死表头零位移(§15.4)`
### Task 4: X5 Windows 三件套(搜索/复制/编辑)
**Files:** AddressBar.tsx;App.tsx;lib/address.ts;zh.json;tests。
- [ ] 搜索 fav 位启用(跨节滤行+计数);复制路径 favdir 核实;编辑 favdir 快捷命名空间语法(解析+回填)
- [ ] vitest+tsc;Commit:`feat(v3p3-rework4): fav 位搜索/复制/编辑三件(快捷命名空间地址,§15.5)`
### Task 5: X6 空目录传输修复
**Files:** 视根因(crates/core 或 daemon)。
- [ ] 复现→根因→修(空目录合法);e2e;Commit:`fix(engine): 空目录传输必败(根因:{一句话},§15.3)`
### Task 6: X7 iOS 全套对齐
**Files:** ios FavoritesMixedView.swift;FavoritesView.swift(树);Kit 若需;tests。
- [ ] 分类恒定(删显示方式/色点)+SWR 缓存;树=成员行(设备名-目录名,§13.8 形态回归);行左滑(下载/转发/删除门);批量核实;搜索框
- [ ] swift test+build;Commit:`feat(ios): 分类恒定+树成员行+左滑/批量/搜索(§16/§15.6)`
### Task 7: X8 回归+重打包+冒烟 §I
- [ ] 四套件;r5 包+TestFlight;smoke §I 重写(I1 分类恒定/I2 树成员节点/I3 组行导航/I4 空目录/I5 列宽/I6 三件套/I7 iOS 补面/I8 全量复走——§G/§H 相应预期同步新形态);spec §16 注记;plan 勾选
- [ ] Commit:`docs(v3p3-rework4): 冒烟 §I+收官注记`
