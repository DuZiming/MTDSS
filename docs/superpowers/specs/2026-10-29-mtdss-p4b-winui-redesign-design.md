# MTDSS Windows UI 改版设计稿(三主题 + 八项体验修复)

- 日期:2026-10-29
- 状态:brainstorm 定稿(视觉六轮 mockup 收口);待 builder check → 用户批准
- 视觉定稿存档:`.superpowers/brainstorm/2186-1793238382/content/visual-style*.html`(v1 方向三选、v2 修订+交互特写、v4 方向列、v6 状态列)
- 范围:**ui/ crate 纯前端**(React/TS/CSS + ui 自身 Tauri shell);daemon/proto/IPC 契约零改动;Android 零改动;golden 零漂移(不触契约面)
- 票据:用户实盘 8 项(2026-10-29 口述)+ 三轮视觉裁定

## 1. 意图与为什么

现有 UI 是 M3a「贴近资源管理器的朴素布局」直系后代:灰白、字符箭头、无文件图标、无拖拽视觉、侧栏滚动会拖动整页。功能已齐(浏览/传输/任务抽屉),观感与手感停在占位水平。本刀把它抬到「日常主力工具」:选定**夜间监控台(暗色)为默认**的完整三主题视觉系统,并修复用户点名的 8 项体验缺陷。改版不新增任何业务能力。

## 2. 目标 / 非目标 / 反目标

| 类 | 内容 |
|---|---|
| 目标 | ① 三主题系统(默认暗色,循环切换)② 侧栏独立滚动 ③ 文件/文件夹图标 ④ 拖拽视觉(ghost+无蓝底)⑤ 地址栏重设计(设备固定 chip+可编辑路径+复制路径)⑥ 工具栏重排(排序/查看在四操作键后,主题按钮最右)⑦ 导航/树/方向等全部字符箭头换 SVG 图标 ⑧ 表头四键点击排序(附:类型列细化、抽屉分栏对齐定稿) |
| 非目标 | 文件重命名/常用目录实体化/按设备开放范围等旧 V2 待办(后续刀);双端进度统一(F5,后续);托盘菜单改动;新增业务功能 |
| 反目标 | 动 daemon/proto/IPC 契约;引入外部字体/图标库/网络资源(严格 CSP);「新建(V2 占位)」按钮被删(用户钦定保留);暗色下键盘焦点环/可达性回退 |

## 3. 约束

- **严格 CSP(M4a 硬化)**:图标全部内联 SVG(React 组件),字体用系统字体(Segoe UI / Segoe UI Variable),零外部请求
- 现有架构沿用:App(状态中枢)/Toolbar/AddressBar/Sidebar/FileDetails|FileList|FileTiles/TransferDrawer/StatusBar;纯逻辑在 `lib/`(vitest 单测文化)
- 行为契约保持:指针拖拽命中测试(`data-drop-loc`)、模态门控(`modalBusy`)、选择/框选/Shift 范围、导航历史栈、抽屉开合与拖高记忆、`nav.ts` 路径分隔符语义(严禁以单一 `/` 切分)
- i18n:zh.json 增量键;词汇沿用现有(接收/发送/调度/进行中/完成/失败/已取消/排队中)

## 4. 主题系统(本刀核心基建)

- `:root[data-theme="fluent|workbench|dark"]`;**默认 `dark`**;持久化 `localStorage["mtdss.theme"]`(无记录=dark);启动即用,无闪烁(挂载前同步设置 data-theme)
- 工具栏最右**主题循环按钮**(月亮/对比度图标+右下角点),点击 `fluent→workbench→dark→fluent`;tooltip=主题名;aria-label 同步
- 窗口标题栏:尽力随主题调 Tauri window theme(dark↔light);不支持则静默跟随系统(非阻塞项)
- 三主题 token(源自定稿 mockup,落 `:root` 变量,全组件只消费变量):

| token | A fluent 原生 | B workbench 工具台 | C dark 夜间监控台 |
|---|---|---|---|
| --bg | #F3F3F6 | #FBFBF9 | #141519 |
| --surface | #FFFFFF | #FFFFFF | #1B1D22 |
| --chrome | #FAFAFC | #F6F6F2 | #17181D |
| --text | #1F1F26 | #191A1B | #E8E9EC |
| --muted | #71717E | #84847C | #9B9DA8 |
| --hair(区分隔线) | #E9E9EF | #E5E5DE | #2B2E36 |
| --rowhair(行分隔) | #F4F4F8 | #F1F1EA | #232529 |
| --winedge | #E2E2E9 | #D9D9D0 | #0C0D10 |
| --link | #0F6CBD | #274690 | #8AB8DC |
| --accent | #0F6CBD | #274690 | #E0A63C(琥珀) |
| --sel / --selhov | #EAF2FB / #DDEBF9 | #E9EDF8 / #DFE6F6 | #2A2D3A / #313444 |
| --hover | #F2F2F6 | #F4F4EE | #22242B |
| --rctl(控件圆角) | 6px | 5px | 6px |
| --rowh(文件行高) | 32px | 26px | 30px |
| --fs(正文字号) | 12.5px | 12px | 12.5px |
| --scroll | #D9D9E3 | #CFCFC4 | #3A3D46 |
| --ib / --iboff(图标钮) | #4A4A55 / #B8B8C2 | #3B3C3E / #B9B9B0 | #C6C8D2 / #5C5F6A |
| --ok | #0F8A4D | #0F7B3F | #57B77E |
| --devbg(设备 chip 底) | #EDF2FA | #EDEFE4 | #26282F |

- 图标着色同为变量:`--icon-folder-fill/edge`、`--icon-file-body/edge`、七个类型 tint(pdf #D9483B / xls #217346 / vid #8E44AD / exe #5A6B7D / txt #8A8F98 / img #2F7EB5 / 压缩包 #C77E14,三主题共用;tint 底色按主题取值——A 淡染页身、B 白页身、C #26272C 页身,参照存档)。组件内**禁止硬编码颜色**
- **主题化覆盖全部表面**:除 §5-§11 逐条列举的区域外,ConfirmDialog / ConflictDialog / ContextMenu / GuideView(登录页,高频大表面)/ UpdatePage / offline-banner / op-notice / NotifyToasts / 滚动条同样退役硬编码色、只消费 token(App.css 现存 ~150 处硬编码色值全量迁移);每主题新增 `--focus`(焦点环,与底对比 ≥3:1)
- 主题启动无闪烁尽力项:`main.tsx` 在 `createRoot` 渲染前同步读 localStorage 设 `data-theme`(CSP 不放行内脚本,非默认主题冷启动残余一帧闪烁可接受,不为之下调 CSP)
- 徽章语义色(抽屉 类型/状态)每主题一组,语义角色恒定;C 组以 mockup 为准(接收=琥珀系、发送=蓝系 #7FB2E8、调度=紫灰系 #AC9CE0、完成=--ok、失败=#E07B70 系、中断=--muted 系);A/B 由各自 accent/中性派生(视觉存档 v1 可参照)
- 选中行:通用底色 --sel;B/C 另有左缘 2px --accent 内嵌条(视觉存档);焦点环 token 每主题保证 ≥3:1 对比

## 5. 布局与滚动(修「整页跟着动」)

- `.sidebar` 补 `overflow-y:auto` + `min-height:0`;滚动条细样式(webkit-scrollbar 5px,--scroll 色)
- 其余滚动行为不变(`.main` overflow:auto、表头 sticky 已有);`html/body` 禁溢出(现状保持)

## 6. 工具栏

顺序:`新建(V2 占位,disabled,样式随主题)` | 分隔 | `剪切 复制 粘贴 删除`(纯图标 30×28,tooltip=现 i18n 词,启用态沿现门控) | 分隔 | `排序 查看`(图标+文字按钮,下拉菜单沿现 MenuGroup 机制) | 弹簧 | `主题循环按钮`
- **排序菜单扩四键**:名称/修改日期/类型/大小 × 递增/递减,与表头点击共享同一 `{key,dir}` 状态源(菜单是 list/tiles 视图与键盘路径的入口)
- 操作按钮图标化后保持 aria-label/tooltip;禁用态 --iboff

## 7. 地址栏(重设计)

自左向右:`[←][→][↑]`(SVG 图标按钮,禁用灰)| **设备 chip** | 面包屑 | `复制路径`(纯图标)| 搜索框(不变)。

- **设备 chip(固定)**:当前设备图标(PC/手机形)+设备名+chevron;**浏览期间恒定**,后退/前进/上级均不换设备;点击弹下拉:全部设备(在线正常、离线灰显禁选)+当前项勾选;选中=进入该设备根(path=""),入导航历史。设备层(kind=devices)时 chip 呈「设备列表」灰态,下拉仍可直达设备根。
- **面包屑**:现有段逻辑(pathSegments/locAtSegment/点击导航/`data-drop-loc` 拖放落点)全部保留,仅重着色;末段粗体不可点。
- **复制路径**:复制当前目录的**原生完整路径**(如 `D:\工作文档\项目源码`;卷根=如 `D:\`);设备层**与卷列表位(path="")均禁用**(卷列表无原生路径);成功弹轻 toast「路径已复制」(5s 自隐,复用现 toast 队列)。实现优先 `navigator.clipboard.writeText`;WebView2 上下文若不可用则在 ui 自身 Tauri shell 加一条最小复制命令(不触 daemon)。
- **编辑态**(进入:点击面包屑空白处 / F4 / Ctrl+L):crumbbox 整体变输入框,预填当前完整路径并全选;**回车=解析跳转;Esc 或失焦=还原**。解析规则(新纯函数 `parseAddressInput`,放 lib/,带单测):
  1. trim 后匹配 `^\\+([^\\/]+)[\\/](.+)$`:首段作设备名候选,**匹配优先级=设备 id 精确 > 设备名唯一匹配(大小写不敏感)> 多个同名匹配(报歧义 toast「存在多台同名设备,请从设备列表选择」,不跳转)**→ 命中=切该设备+余段为路径;未命中=toast「未找到设备:X」,不跳转。**裸 `\设备名`(无路径段)=进入该设备根**(卷列表位)
  2. 否则视为当前设备路径:混用 `/`、`\` 一律归一为该设备平台原生分隔符(nav.ts 语义);非绝对(无盘符/无根前缀)且当前在目录位=与当前目录拼接;**设备层时任何无设备前缀的输入(相对或绝对形)一律 toast 提示需带设备前缀或先进设备**(键 `address.needDeviceOrAbsolute`)
  3. 跳转走现有 navigate;目标不存在/离线由现有浏览错误态自然呈现(「加载失败:…」+重试)
- 编辑态期间地址栏不响应拖放落点(输入框优先);模态门控(modalBusy)打开时不进入编辑态

## 8. 文件列表与表头排序

- 排序状态 `{key:"name"|"mtime"|"type"|"size",dir:"asc"|"desc"}`(App 持有,替换现 sortDir);`sortEntries` 扩 key 参数;**目录恒在最前(全键、双向皆然)**;表头(details 视图)可点:同键翻转方向、新键默认升序;活动列实心 ▲/▼,其余列悬停淡 ↕;三视图共享排序结果(list/tiles 无表头,经工具栏菜单)
- **type 键比较裁定**(builder check 裁定):主键=类型显示名(与类型列文案同源,zhCmp 比较,同类型自然聚组);同值次键=名称升序(**不随 dir 反转**);目录恒前同样不反转——desc 只反转主键方向
- **类型列细化**(纯 UI 派生):目录=「文件夹」(不变);已知扩展名走策展映射(pdf→PDF 文档、xls/xlsx→Excel 工作表、doc/docx→Word 文档、mp4/mkv/mov…→视频、mp3/flac…→音频、jpg/png/gif…→图片、zip/7z/rar→压缩包、exe/msi→应用程序、txt/md→文本文档);未知=`「{{ext 大写}} 文件」`;新 i18n 键
- **行图标**(16px,三视图统一):文件夹图标 / 文件页形+类型 tint 角标(§4 变量);名称列=图标+8px 间距;大小列右对齐 `font-variant-numeric:tabular-nums`
- 设备表(DeviceTable)名称列加平台图标(PC/手机)

## 9. 拖拽视觉(修「文件不动+蓝底」)

现有指针拖拽机制(4px 阈值/pointer capture/吞 click/data-drop-loc 命中)**零改动**,只补视觉:

- 行与视图宿主 `user-select:none`(选中态本就自绘,原生文本选中彻底退役)
- 拖拽成立:被拖行集半透明(opacity .55);**ghost 浮卡**跟随光标(fixed 定位,pointermove transform 移动,不触发重排):首项图标+名称,多选加「N 项」角标;grabbing 光标(现状)
- 落点悬停高亮(树节点/面包屑/当前目录)重样式:--accent 描边+淡底;语义不变(仍 data-drop-loc)
- 拖出窗口语义不变(drop_resolve_at_cursor)

## 10. 传输抽屉

行为(开合记忆/拖高/筛选四档/操作可用性矩阵/控制 RPC)**零改动**,重排呈现:

- **收起条 grid**:`折叠钮 | 类型徽章 | 任务名·对端→目标(1fr)| 进度条 150px | 百分比 48px | 速度 88px | 操作(max-content,靠右)| 计数徽章`。**内容规则(用户裁定 2026-10-29)**:显示**最新活动任务**(queued/running/paused 中最近更新者);无活动=最新终态任务(进度条与百分比位显示状态词);完全无任务=「传输 · 暂无传输任务」;**计数徽章**保留聚合可视信号——非零段显示「进行中 N · 中断 N · 失败 N」,零段隐藏,全零整徽章隐藏
- **展开表六列定比例**:任务名 22% / 类型 9% / 方向 12% / 状态 34% / 传输速度 11% / 操作 12%(右对齐);`table-layout:fixed`
- **类型列**=三色徽章(§4 徽章语义色)
- **方向列**=图标+文字,靠左:接收=箭头落托盘、发送=箭头出托盘、调度=双向交换(定稿 SVG 见存档 v4);本机复制(local_copy)/接收(incoming)归入「其他」徽章的现状保留,方向图标按流向归并(builder 按现有 kind 集合映射,缺者用 swap)
- **状态列三段定宽,跨行严格对齐**:徽章段 52px(文字居中,「进行中/已取消」同宽)| 槽段 160px | 百分比段 38px(右对齐 tabular)。槽内:进行中与**已暂停均渲染进度条布局**(暂停徽章已表意,进度照显);**完成=满进度条(--ok 绿)+「100%」**;失败/取消/中断=**原因文字左对齐占同一槽**(错误串截断省略,无错误串用状态词;颜色 --muted,红色语义由徽章承担);排队中=「排队中」文字
- 摘要/速率 tabular 数字;状态栏右段速率同步 tabular

## 11. 侧栏树

- caret 换 SVG chevron(▸,展开旋转 90°,--muted);节点加 16px 文件夹图标;设备节点=平台图标+名+在线点(--ok,现状语义);「常用目录 V2 占位」保留 disabled
- 滚动修复见 §5;选中/hover/错误节点样式随主题

## 12. i18n 增量键(清单)

`theme.fluent/workbench/dark`、`theme.cycle`(含当前名)、`address.copyPath`、`address.pathCopied`、`address.deviceList`、`address.deviceNotFound`、`address.needDeviceOrAbsolute`、`toolbar.sortKey.name/mtime/type/size`(排序菜单四键)、`files.type.pdf/xls/doc/video/audio/image/archive/app/text/unknownFile`、`drawer.dir.recv/send/ctl`(方向列文字+tooltip)。其余沿用。

## 13. 测试与验收

- **纯函数单测(vitest)**:`parseAddressInput`(设备前缀命中/未命中、分隔符归一、相对拼接、设备层相对输入)、`sortEntries` 多键+目录恒前、类型派生映射、主题循环与持久(若提炼 reducer);受影响的既有 145 用例改齐
- 组件行为既有测试面回归;无新增 e2e(UI 无此档)
- **手动冒烟清单(入 plan)**:三主题切换+持久+默认 dark,且**对话框(删除确认/冲突)/登录引导页/更新页/右键菜单/toast 各表面三主题过目**;侧栏长列表滚动隔离(主区不动);表头四键排序+菜单同步;拖拽 ghost/半透明/无蓝底/落点高亮/拖出;复制路径(实粘到资源管理器验证);编辑态:粘贴原机路径回车跳转、`\设备名\路径` 切设备、裸 `\设备名` 进设备根、同名设备歧义 toast、Esc/失焦还原、错路径错误态;抽屉:三段对齐、完成满条、失败原因对齐、收起条分栏;键盘焦点环三主题过目
- 验收=用户实机过目(「改完我们再调整」迭代回路,后续微调按票走)

## 14. 交给实现者的余地

- 图标具体笔画(存档为基准,同语义同风格可微调)、A/B 徽章色板派生、ghost 浮卡的阴影/圆角细节、抽屉列宽 ±2% 微调、类型映射词表扩展
- 复制路径的桥命令与 clipboard API 二选一(以实机可用为准)
- 标题栏主题跟随的实现深度(尽力项)
