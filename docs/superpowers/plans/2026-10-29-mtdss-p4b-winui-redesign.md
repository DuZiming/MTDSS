# MTDSS P4b Windows UI 改版 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 按 [设计稿](../specs/2026-10-29-mtdss-p4b-winui-redesign-design.md) 完成桌面端三主题视觉系统(默认暗色)+ 用户 8 项体验修复,纯 ui/ crate,契约零改动。

**Architecture:** 先立主题 token 基建(themes.css + 启动无闪烁 + 循环按钮),再落纯逻辑层(排序多键/类型词表/地址解析/收起条选任务),图标库先行供各组件消费;组件任务(地址栏/工具栏/三视图/侧栏/抽屉/拖拽)各自携带本区域 CSS 的 token 迁移,最后一批任务清剩余表面(对话框/引导页/更新页/通知等)并收官终审。

**Tech Stack:** React 18 + TypeScript + Vite + Tauri 2(WebView2);vitest 纯函数单测;全部图标内联 SVG(严格 CSP,零外部资源);系统字体 Segoe UI。

**Spec:** `docs/superpowers/specs/2026-10-29-mtdss-p4b-winui-redesign-design.md`(本计划一切数值/规则以 spec 为准,冲突时以 spec 为准)

## Global Constraints

- **CSP 严格(M4a)**:零外部请求——图标全部内联 SVG 组件,字体只系统字体;禁止引入图标库/字体/网络资源
- **契约零改动**:daemon/proto/IPC 不动,golden 零漂移;唯一允许的 Rust 面是 ui 自身 Tauri shell 的 `copy_text` 命令(T4)
- **禁止硬编码颜色**:所有颜色只消费 CSS 变量(App.css 现存 ~150 处硬编码全量退役);图标着色走 `--icon-*` 变量
- **路径纪律(nav.ts 模块告诫)**:严禁以单一 `/` 切分或以 `\` 拼装;一律经 nav.ts 语义
- **「新建(V2 占位)」按钮保留**(disabled,样式随主题)
- i18n:zh.json 增量键,词汇沿用现有(接收/发送/调度/进行中/完成/失败/已取消/排队中)
- 既有 145 个 vitest 用例全绿回归;每任务尾跑 `npm -C ui test`(必要时 `npm -C ui run build` 过 tsc)
- 执行环境:worktree 分支(worktree-mtdss-p4b-winui);UI 门禁= vitest 全量 + tsc/build 通过

## Review Focus

1. **混合分隔符粘贴路径**(如 Windows 设备上粘 `D:/工作/项目 源码` 含 `/` 与空格)→ 归一为原生 `\` 后跳转成功 → `address.test.ts` 用例钉住
2. **同名设备前缀歧义**(`\主机\path` 匹配两台)→ 返回歧义错误,UI toast 不跳转 → `address.test.ts`
3. **type 键 desc 排序**:目录恒最前不反转、组内按类型显示名反序、同类型内名称恒升序 → `browse.test.ts` 追加
4. **收起条选任务**:多活动取最后更新的活动任务;全终态取最后终态;空 → null(渲染「暂无传输任务」)→ `transfer.test.ts` 追加
5. **主题持久化脏值**:localStorage 存了非法值/空 → 冷启动兜底 dark 不抛 → `theme.test.ts`
6. **表头切换排序键**:点击新列重置为 asc、同列翻转、状态与工具栏菜单双向同步 → `browse.test.ts` 纯函数部分 + T6 手检清单条目

---

### Task 1: 主题基建(token 表 + 无闪烁启动 + 循环按钮)

**Files:**
- Create: `ui/src/themes.css`
- Create: `ui/src/lib/theme.ts`
- Modify: `ui/src/main.tsx`
- Modify: `ui/src/App.tsx`(theme 状态 + 传参)
- Modify: `ui/src/components/Toolbar.tsx`(最右循环按钮)
- Test: `ui/src/lib/theme.test.ts`

**Interfaces:**
- Produces: `type ThemeId = "fluent" | "workbench" | "dark"`;`const THEMES: ThemeId[]`(循环序 fluent→workbench→dark);`const DEFAULT_THEME: ThemeId = "dark"`;`const THEME_STORAGE_KEY = "mtdss.theme"`;`nextTheme(t: ThemeId): ThemeId`;`loadTheme(): ThemeId`(缺失/非法→DEFAULT);`saveTheme(t: ThemeId): void`(try/catch 包裹)
- Produces: `:root[data-theme="fluent|workbench|dark"]` 三组 token(spec §4 全表逐值落,含 `--focus` 与图标变量 `--icon-folder-fill/edge`、`--icon-file-body/edge`、七个类型 tint `--icon-tint-pdf/xls/vid/exe/txt/img/archive`);Toolbar 新增 prop `theme: ThemeId; onCycleTheme(): void`
- Produces: 标题栏尽力项——App theme effect 内经 `getCurrentWindow().setTheme("dark"|"light")` 跟随(dark 主题→dark,fluent/workbench→light);API 不支持或抛错时静默吞,不阻塞
- Consumes: App 持有 `useState<ThemeId>(loadTheme())`,effect 同步 `document.documentElement.dataset.theme` + `saveTheme`

- [ ] **Step 1: 写失败测试** `theme.test.ts`:`nextTheme` 三向循环回绕;`loadTheme` 无记录/非法值(`"neon"`)/空串 → `"dark"`;`saveTheme`→`loadTheme` 回读(测试尾清 localStorage)
- [ ] **Step 2: 跑红** `npm -C ui test -- theme` → FAIL(模块不存在)
- [ ] **Step 3: 实现 `lib/theme.ts`**(localStorage 访问全 try/catch);**`themes.css`** 三组 token 逐值抄 spec §4 表(tint 底色三主题各自取值:A 淡染页身/B #FFFFFF/C #26272C,folder/tint 色值同表);`main.tsx` 在 `createRoot` 前 `document.documentElement.dataset.theme = loadTheme()` 并 import themes.css
- [ ] **Step 4: 跑绿** + Toolbar 末尾加循环按钮(`ThemeIcon` 先用占位字符 ◐,T3 换 SVG;aria-label=`t("theme.cycle")`)+ App 接线
- [ ] **Step 5: 全量 vitest + build 过;Commit** `feat(ui): 三主题 token 基建+无闪烁启动+循环切换(P4b)`

### Task 2: 排序多键与类型词表(纯逻辑)

**Files:**
- Modify: `ui/src/lib/browse.ts`(sortEntries 扩 key)
- Create: `ui/src/lib/filetype.ts`
- Test: `ui/src/lib/browse.test.ts`(追加)、`ui/src/lib/filetype.test.ts`(新)

**Interfaces:**
- Produces: `type SortKey = "name" | "mtime" | "type" | "size"`;`interface SortState { key: SortKey; dir: "asc" | "desc" }`;`sortEntries(es: FsEntry[], state: SortState, cmp: Cmp): FsEntry[]`(破坏性签名替换,调用点本任务内改齐:App.tsx rows memo、Sidebar.tsx `{key:"name",dir:"asc"}`)
- Produces: `fileTypeName(name: string, isDir: boolean): string`(zh 词表单源于此;spec §8 策展映射:PDF 文档/Excel 工作表/Word 文档/视频/音频/图片/压缩包/应用程序/文本文档;未知→`"${ext 大写} 文件"`;目录→「文件夹」);`fileTypeTintKey(name: string, isDir: boolean): "folder" | "pdf" | "xls" | "vid" | "exe" | "txt" | "img" | "archive" | "generic"`(图标角标取色键,策展映射同源;目录→folder,未知→generic[tint 用 --muted 系]);zh.json `files.type.dir/file` 旧键在 T6 退役
- 排序规则(spec §8 裁定):目录恒最前(全键不反转);type 键主比 `fileTypeName` 显示名(cmp)、次键名称**恒 asc**;desc 仅反转主键

- [ ] **Step 1: 失败测试**:sortEntries 四键×升降(含目录恒前、type 次键名称 asc 不随 dir 反转、mtime_ms/size 数值比较);fileTypeName 策展命中(大小写扩展名 `.PDF`)/未知(`.zzz`→`ZZZ 文件`)/目录;fileTypeTintKey 同输入集(folder/pdf/…/generic)
- [ ] **Step 2: 跑红** → **Step 3: 实现**(cmp 复用 zhCmp 入参)→ **Step 4: 跑绿 + 改齐两处调用点(行为等价 `{key:"name",dir:sortDir}`)+ 全量 vitest**
- [ ] **Step 5: Commit** `feat(ui): 排序四键多列+类型词表纯逻辑(P4b)`

### Task 3: 图标组件库

**Files:**
- Create: `ui/src/components/icons.tsx`
- Test: `ui/src/lib/icons.test.tsx`(renderToStaticMarkup 冒烟)

**Interfaces:**
- Produces(均 `({size?: number}) => JSX.Element`,stroke=currentColor 线性族;基准笔画以 spec 存档 v2/v4 为准):
  导航 `IcBack/IcForward/IcUp/IcChevronRight/IcChevronDown`;操作 `IcCut/IcCopy/IcPaste/IcDelete`;菜单 `IcSort/IcView`;主题 `IcTheme`(月亮+三点,存档 v2);地址 `IcCopyPath/IcSearch`;平台 `IcPc/IcPhone`;抽屉操作 `IcPause/IcX/IcRedo/IcTrash`;杂项 `IcCheck`
  着色 `IcFolder`(fill=var(--icon-folder-fill),stroke=var(--icon-folder-edge))、`IcFile`(页形 body/edge 变量 + 角标 rect fill=按 tintKey 取 var)、`IcFileTyped(tintKey: "pdf"|…|"archive")`
  方向 `IcDirRecv/IcDirSend/IcDirSwap`(托盘语义,存档 v4 笔画)

- [ ] **Step 1: 失败测试**:每个导出 renderToStaticMarkup 含 `<svg`;IcFileTyped("pdf") 含 `var(--icon-tint-pdf)`
- [ ] **Step 2: 跑红 → Step 3: 实现(逐图标照存档笔画,不自创)→ Step 4: 跑绿 + build**
- [ ] **Step 5: Commit** `feat(ui): 内联 SVG 图标组件库(P4b)`

### Task 4: 地址栏重设计(chip+面包屑+复制路径+编辑态)

**Files:**
- Create: `ui/src/lib/address.ts`
- Modify: `ui/src/lib/nav.ts`(导出 `splitPath`)
- Modify: `ui/src/components/AddressBar.tsx`(整体重写)
- Modify: `ui/src/App.tsx`(props 接线:devices/selfDeviceId 已有;编辑态解析回调)
- Modify: `ui/src-tauri/src/lib.rs` + `ui/src/lib/daemon.ts`(桥命令 `copy_text`)
- Modify: `ui/src/App.css`(地址栏区域 token 迁移)
- Test: `ui/src/lib/address.test.ts`

**Interfaces:**
- Produces: `type AddressTarget = { kind: "nav"; device: string; path: string } | { kind: "deviceRoot"; device: string } | { kind: "error"; code: "deviceNotFound" | "deviceAmbiguous" | "needDeviceOrAbsolute"; name?: string }`
- Produces: `parseAddressInput(input: string, devices: Array<{device_id: string; name: string | null; platform: string}>, current: { device: string; path: string } | null): AddressTarget`(spec §7 全规则:前缀 `^\\+([^\\/]+)[\\/](.+)$` id 精确>名唯一>歧义;裸 `\设备名`=deviceRoot;分隔符归一为目标设备平台原生;相对=与 current.path 拼接;current=null 时无前缀一律 needDeviceOrAbsolute)
- Consumes: T3 `IcBack/IcForward/IcUp/IcChevronRight/IcChevronDown/IcCopyPath/IcSearch/IcPc/IcPhone/IcCheck`
- AddressBar props 增:`devices: DeviceInfo[]; onCycleTheme 无关`;`onAddressSubmit(target)` 由 App 转 navigate/toast;复制路径经 `daemonCopyText(text)`(navigator.clipboard.writeText 失败 catch → invoke `copy_text`)

- [ ] **Step 1: 失败测试**(spec Review Focus #1/#2 + 边角):混合分隔符归一(Windows 目标 `/`→`\`,POSIX 目标 `\`→`/`);id 命中/名唯一命中/两名歧义;裸前缀 deviceRoot;相对拼接(目录位);设备层无前缀绝对形→needDeviceOrAbsolute;空串 trim
- [ ] **Step 2: 跑红 → Step 3: 实现 address.ts + nav.ts 导出 splitPath;AddressBar 重写**(设备 chip+下拉[离线灰显/当前勾选/点击=进设备根入历史]、面包屑沿用 pathSegments/locAtSegment/data-drop-loc、复制路径按钮纯图标[设备层与 path="" 禁用,成功 toast `address.pathCopied`]、编辑态[点空白/F4/Ctrl+L 进入,预填全路径全选,Enter 提交→parse→nav 或 toast,Esc/失焦还原;modalBusy 时不进入];设备层 chip=「设备列表」灰态)
- [ ] **Step 4: 桥命令 `copy_text(text: String) -> Result<()>`(Rust 侧 `set_clipboard` 或等价;daemon.ts 包装 `daemonCopyText`);跑绿 + build + 本区域 CSS token 化**
- [ ] **Step 5: Commit** `feat(ui): 地址栏重设计——设备 chip/复制路径/可编辑跳转(P4b)`

### Task 5: 工具栏重排与排序菜单四键

**Files:**
- Modify: `ui/src/components/Toolbar.tsx`
- Modify: `ui/src/App.tsx`(`sortDir` → `sortState: SortState`)
- Modify: `ui/src/App.css`(工具栏区域)
- Test: 纯逻辑已在 T2;本任务手动验证 + 全量回归

**Interfaces:**
- Consumes: T2 `SortState/SortKey`;T3 `IcCut/IcCopy/IcPaste/IcDelete/IcSort/IcView/IcTheme`
- Produces: Toolbar props 改 `sortState: SortState; onSortState(s: SortState)`(替换 sortDir/onSortDir);主题按钮自 T1 位置不变

- [ ] **Step 1: Toolbar 重排**:`新建(V2 占位保留 disabled)|分隔|剪切 复制 纯图标 粘贴 删除(纯图标,aria-label=现 i18n,启用门控不变)|分隔|排序 查看(图标+文字)`;弹簧后主题循环按钮换 `IcTheme`
- [ ] **Step 2: 排序菜单两组 radio**:「排序方式」名称/修改日期/类型/大小 + 「方向」递增/递减(键 `toolbar.sortKey.*`;与 T6 表头同一 SortState 状态源;Escape/外点收起沿用)
- [ ] **Step 3: 全量 vitest + build;目检三主题工具栏**
- [ ] **Step 4: Commit** `feat(ui): 工具栏重排+四键排序菜单(P4b)`

### Task 6: 文件三视图与设备表(表头排序/行图标/类型列)

**Files:**
- Modify: `ui/src/components/FileDetails.tsx`、`FileList.tsx`、`FileTiles.tsx`、`DeviceTable.tsx`
- Modify: `ui/src/App.tsx`(dirProps 增 sortState/onSortKey)
- Modify: `ui/src/App.css`(三视图+设备表区域)
- Test: `ui/src/lib/browse.test.ts` 追加表头状态纯函数(如无可省)

**Interfaces:**
- Consumes: T2 `fileTypeName/fileTypeTintKey/SortState`;T3 `IcFolder/IcFileTyped/IcPc/IcPhone`
- Produces: `headerClick(state: SortState, key: SortKey): SortState`(同键翻转/新键 asc——若 App 内联则不导出)

- [ ] **Step 1: FileDetails 表头**:四列 th 可点(同键翻 dir/新键 asc),活动列 `▲/▼`(IcChevronDown 旋转或三角 SVG,用图标不用字符),hover 淡指示;`aria-sort` 标注
- [ ] **Step 2: 行图标+类型列**:名称列 `IcFolder`/`IcFileTyped`(tintKey 由扩展名映射);类型列 `fileTypeName`(zh.json files.type.dir/file 旧键退役);大小列右对齐 `font-variant-numeric: tabular-nums`;行高 `var(--rowh)`;FileList/FileTiles 同图标;DeviceTable 名称列 `IcPc/IcPhone`(platform: windows/linux/macos→Pc,android→Phone)
- [ ] **Step 3: CSS 区域 token 迁移 + 全量 vitest + build;目检(排序/图标/三视图)**
- [ ] **Step 4: Commit** `feat(ui): 三视图图标化+表头四键排序+类型列细化(P4b)`

### Task 7: 侧栏树(图标 caret+节点图标+滚动修复)

**Files:**
- Modify: `ui/src/components/Sidebar.tsx`
- Modify: `ui/src/App.css`(侧栏区域)
- Test: 无新纯逻辑;全量回归 + 手动

**Interfaces:**
- Consumes: T3 `IcChevronRight(展开旋转 90°)/IcFolder/IcPc/IcPhone`

- [ ] **Step 1: 实现**:caret 文字符换 SVG(展开态 rotate);目录节点 16px `IcFolder`、设备节点平台图标+在线点(--ok,现状语义);「常用目录 V2 占位」保留 disabled;`.sidebar { overflow-y: auto; min-height: 0 }` + 细滚动条(webkit-scrollbar 5px,var(--scroll))
- [ ] **Step 2: 全量 vitest + build;目检:长树滚动仅侧栏、主区不动**
- [ ] **Step 3: Commit** `feat(ui): 侧栏树图标化+独立滚动修复(P4b)`

### Task 8: 传输抽屉(收起条重排+展开表定比例+方向/状态列)

**Files:**
- Modify: `ui/src/lib/transfer.ts`(收起条选任务纯函数)
- Modify: `ui/src/components/TransferDrawer.tsx`
- Modify: `ui/src/App.css`(抽屉区域)
- Test: `ui/src/lib/transfer.test.ts`(追加)

**Interfaces:**
- Produces: `pickCollapsedJob(jobs: Map<string, JobSnapUI>): JobSnapUI | null`(规则:非终态 queued/running/paused 中取迭代序最后者;无非终态取终态迭代序最后者;空→null)+ `collapsedCounts(jobs): { active: number; interrupted: number; failed: number }`(沿现摘要计数口径)
- Consumes: T3 `IcChevronDown/IcPause/IcX/IcRedo/IcTrash/IcDirRecv/IcDirSend/IcDirSwap`

- [ ] **Step 1: 失败测试**:多活动取最后;活动+终态混合取活动;全终态取最后终态;空 map→null;counts 三段(spec Review Focus #4)
- [ ] **Step 2: 跑红 → Step 3: 实现 pickCollapsedJob/collapsedCounts;TransferDrawer 重排**——收起条 grid `折叠钮|类型徽章|任务名·对端→目标(1fr)|进度条 150px|百分比 48px|速度 88px|操作 max-content 靠右|计数徽章`(内容规则 spec §10 用户裁定:最新活动任务/最新终态任务(状态词占条位)/空=「传输 · 暂无传输任务」;计数徽章非零段「进行中 N·中断 N·失败 N」零段隐藏;操作=展示任务的可用操作沿现矩阵);展开表六列 `22/9/12/34/11/12%` table-layout:fixed 操作右对齐;类型=三色徽章(C 主题色值 spec §4,A/B 由 accent 派生);方向=图标+文字靠左(recv/send/swap,local_copy/incoming 归「其他」徽章+流向映射,缺者 swap);状态三段定宽 `52px 徽章居中|160px 槽|38px 百分比右对齐 tabular`(进行中/已暂停=进度条布局;完成=--ok 满条+100%;失败/取消/中断=原因文字占槽[错误串截断,无则状态词];排队中=文字)
- [ ] **Step 4: 跑绿 + CSS 区域 token 迁移 + build;目检对齐**
- [ ] **Step 5: Commit** `feat(ui): 传输抽屉分栏定比例+方向图标+状态三段(P4b)`

### Task 9: 拖拽视觉(ghost+禁文本选中)

**Files:**
- Modify: `ui/src/App.tsx`(ghost 状态与渲染;机制零改动)
- Modify: `ui/src/App.css`(行/宿主 user-select、ghost 卡、drop-hover)
- Test: 无新纯逻辑;全量回归 + 手动

**Interfaces:**
- Consumes: T3 `IcFileTyped`(ghost 首项图标);现 dragRef/拖拽机制不动

- [ ] **Step 1: 实现**:行与视图宿主 `user-select:none`;dragging 成立时被拖行集 `.drag-source`(opacity .55,App 以 entries 路径集下发类名);ghost 浮卡(fixed,pointermove `transform: translate()` 跟随;首项图标+名称,多选「N 项」角标徽章);`.drop-hover` 重样式=--accent 描边+淡底(语义不变)
- [ ] **Step 2: 全量 vitest + build;目检:拖动无蓝底、ghost 跟手、落点高亮、拖出窗口语义不变**
- [ ] **Step 3: Commit** `feat(ui): 拖拽 ghost 浮卡+禁用原生文本选中(P4b)`

### Task 10: 全表面主题迁移收尾(对话框/引导/更新/通知/右键/状态栏+焦点环)

**Files:**
- Modify: `ui/src/App.css`(剩余全部区域)、`ui/src/components/ConfirmDialog.tsx`、`ConflictDialog.tsx`、`ContextMenu.tsx`、`GuideView.tsx`、`UpdatePage.tsx`、`NotifyToasts.tsx`、`StatusBar.tsx`(仅当需结构微调配合 token;否则纯 CSS)
- Test: 全量回归

**Interfaces:**
- Consumes: T1 全 token(--focus/--scroll/--icon-*)

- [ ] **Step 1: App.css 剩余硬编码色全量退役**(spec §4:全部表面只消费 token;含 offline-banner/op-notice/滚动条/焦点环);grep 验证:`App.css` 与组件内 `#[0-9a-fA-F]{3,8}`/`rgba?\(` 仅允许出现在 `themes.css`(注:rgba 阴影可留在 themes.css 的 token 定义内)
- [ ] **Step 2: 焦点环/键盘可达**:`:focus-visible` 用 var(--focus) 全控件;三主题对比抽查
- [ ] **Step 3: 全量 vitest + build;目检:删除确认/冲突对话框/登录引导页/更新页/右键菜单/toast 三主题**
- [ ] **Step 4: Commit** `feat(ui): 全表面主题迁移+焦点环收尾(P4b)`

### Task 11: i18n 审计、冒烟清单与终审收官

**Files:**
- Modify: `ui/src/i18n/zh.json`(审计:spec §12 键全量在、无死键)
- Create: `docs/smoke-p4b.md`(实机冒烟清单)
- Modify: spec 状态行收官回填

**Interfaces:**
- Consumes: 全部前任务

- [ ] **Step 1: i18n 审计**(spec §12 清单逐键对账;退役键清理:`files.type.dir/file` 等)
- [ ] **Step 2: `docs/smoke-p4b.md` 编制**(spec §13 手动清单逐条成步骤+预期,含三主题各表面/侧栏滚动隔离/表头排序/拖拽/复制路径实粘验证/编辑态全边角/抽屉三段对齐/焦点环)
- [ ] **Step 3: 全量门禁**:`npm -C ui test`(全绿)+ `npm -C ui run build`;UI tsc 零错
- [ ] **Step 4: 整支终审(requesting-code-review)→ PR;实机场(用户在场)按 smoke-p4b 逐项**

---

## 执行注

- 顺序建议 T1→T11 刚性(T2/T3 可并行,T4-T9 各自独立但都消费 T1-T3;T10 依赖全部组件任务完成)
- 每个「目检」步骤 = 实现者本机 `npm -C ui run dev` 起 UI(daemon 不在也可看未登录引导面/主题切换;登录态面留给终局实机场,不阻塞任务完成)
- 实机场重点(spec §13):主题默认 dark 持久、`\设备名\路径` 切设备、混合分隔符粘贴、收起条计数徽章、完成满条、拖拽无蓝底
