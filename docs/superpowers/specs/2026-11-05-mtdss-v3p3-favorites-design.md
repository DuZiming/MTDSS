# MTDSS V3-3 常用目录系统 设计稿

日期:2026-11-05 | 状态:**已实施(2026-11-06,T1-T19 全量落地;真机冒烟 docs/smoke-v3p3.md 挂用户侧)**
来源:V3 伞形 spec(2026-11-04-mtdss-v3-design.md §5/§9/§12)的 V3-3 波细化;2026-11-05 brainstorm 用户裁定:①混合视图与分组视图**可切、默认混合**(查看处选);②创建/攒成员**两步走**(常用区新建空目录 → 设备浏览中选目录添加进既有常用,可多选);③离线成员呈现面确认;④资源管理器**外部拖入常用视图同受理**(builder 评审轮裁定)。其余工程面为记录人裁量(§10),交审一并过目。
基准:main@57bc874(V3-2 已并)。

## 1. 意图与范围

把伞形 §5 常用目录系统落成可实施设计:**账号级同步的常用清单 + 三端消费面(本波=Windows/iOS,安卓随 V3-4)+ 终端标志色 + 收件箱置顶**。范围=Windows UI(Tauri)+ iOS UI + MTDSSKit + daemon 事件接线 + IPC golden;**服务端零改**(V3-1 已交付 account_kv REST 与 data_changed 通告);不含安卓 UI(V3-4 装入)、不含缩略图/预览(伞形 §10 探索)、不含手机端「放入」场景(伞形非目标)。

## 2. 现状盘点(2026-11-05 核,文件锚)

- **KV 地基(V3-1 已交付)**:`account_kv` REST + daemon IPC `kv.get/kv.put`(api.rs:102-103;value ≤256KiB 服务端校验,api.rs:726);PUT 成功→服务端对本账号其他在线端 `try_send AuthNotice{kind:"data_changed", data_key}`;daemon hub.rs:585 收到 data_changed **仅留痕**(「V3-3 消费面」注释即本波)。
- **Windows UI**:侧栏「常用目录」灰占位(Sidebar.tsx:229 + zh.json sidebar.favorites/favoritesTag);Loc = `{devices} | {device,path}`(nav.ts,路径分解单源);查看菜单三态 tiles/list/details(Toolbar.tsx:203 viewSections/MenuSection);拖放=指针拖拽 + `[data-drop-loc]` 命中测试(树节点/面包屑/当前目录宿主,App.tsx:1414-1472);目录行/多选动作集=剪切/复制/粘贴/刷新/打开/下载到本机/删除(ContextMenu.tsx;delete 走回收站确认梯);多选跨设备传输既有分组语义(transfer.start 每设备每票);未登录=GuideView 全屏。
- **iOS**:「常用」标签=FavoritesView,收件箱单枚根行(Ruling N;AuthorizedRoot.roots 单源,BrowseViewModel.swift:60-63),根行内展开/选择模式/滑删动作集全复用 BrowseViewModel;设备标签=DevicesView→RemoteBrowseTarget 远端浏览;MTDSSKit Contract.swift 已有 `Ops.kvGet/kvPut`(748-753)与 IpcEvent.parse(563-655,**无 data_changed case**);EventHub 扇出;本机浏览保留别名 `self`(Contract.selfDevice,T13b 修#4)。
- **契约纪律**:IPC golden 单一真源 `crates/ipc/contract/`,三端契约测试锁定,变更走 REGEN;`event.devices_changed {"params":{}}` 为新事件面先例。

## 3. 数据模型与同步

### 3.1 schema(kv 键 `favorites`,ASCII 定死)

```json
{
  "v": 1,
  "updated_at": 1730000000000,
  "favorites": [
    {
      "id": "3f9c2a71b04e5d18",
      "name": "工作资料",
      "created_at": 1730000000000,
      "members": [ { "device": "<device_id>", "path": "D:\\工作" } ]
    }
  ]
}
```

- id=客户端生成 16 hex 随机;**成员只存 device_id + path 原串**——设备名/平台/在线态/标志色全部渲染时从设备快照 join,不入库(设备改名零迁移)。
- 顺序:常用条目=创建序;成员=添加序(即分组模式节序)。均不做手动排序。
- 空成员(0 个)合法;同 `(device,path)` 重复添加=静默去重;收件箱路径可作普通成员。
- 删除常用/移除成员=**解除链接,绝不触碰任何远端文件**(删除确认弹窗明示此语义)。

### 3.2 同步协议(整单 LWW)

- 拉取时机:登录成功 / 上线(WS 连接建立,**含断线重连**——data_changed 尽力推送只达在线端,离线窗口的对端编辑靠重连全量拉兜底)+ 收到 `event.data_changed{key:"favorites"}` → `kv.get` 全量替换本地。
- 编辑流程:`kv.get` 取最新 → 本地改(增删改)→ `kv.put` 整单回写。服务端盲覆盖=**整单 LWW、无 CAS**;两台在线端同时编辑=后写者胜,先写者的改动丢失——单用户小规模接受(裁量 #2)。
- **编辑失败语义(服务端为准)**:put 成功才落本地缓存;put 失败(网络/会话失效)=toast 报错、本地保旧值、不自动重试(用户重试动作);编辑前置 kv.get 失败=本次编辑动作失败。kv op 走 REST 只要求会话不要求 WS 在线——信令离线但会话在=允许编辑(裁量 #16)。
- `updated_at` 仅排障字段,非权威;404(无此键)=空清单合法态(not_found typed 错误映射已在,kv.get golden notfound 变体)。
- **前向守卫**:解析出 `v` > 已知(1)→ 本地只读:不渲染编辑入口、提示升级,且**绝不回写**(防旧版 schema 覆盖新版数据)。

## 4. 终端标志色

- **固定 6 色板**(避开 蓝=进行中/绿=完成/红=失败/琥珀=警告 色域;具体值随 spec 交审过目):

| # | 名 | 值 |
|---|---|---|
| 0 | 青 | `#14B8A6` |
| 1 | 天青 | `#06B6D4` |
| 2 | 紫 | `#8B5CF6` |
| 3 | 洋红 | `#D946EF` |
| 4 | 粉 | `#EC4899` |
| 5 | 黛 | `#64748B` |

- **分配算法**:设备快照按 device_id 字典序排序,位次 i → 色板[i mod 6]。零服务端状态、三端同算法同结果;账号 ≤6 台必不撞色;>6 台回绕撞色(叠现接受——device_limit 默认 3,超 6 属运营自定)。设备增删会移位换色:设备增删=稀有账号管理事件,接受(裁量 #3)。两端设备快照同源(daemon devices()=服务端全账号目录含在线位,TS DeviceInfo/Swift DirectoryDevice 字段一致),瞬时陈旧可接受。
- **用法**:混合视图**行首来源色点**(同色=同源);分组模式节头色点;管理屏成员行色点;侧栏/常用页常用条目行的成员色点串。行级着色形态=行首色点的轻方案,不做整行铺色(裁量 #3b,终审可调)。
- 混合视图同名文件=**两行并存,不合并**,靠色点区分(伞形原意)。

## 5. Windows 落地

### 5.1 侧栏常用区(Sidebar.tsx 占位点亮)

- 结构:`「常用目录」分组标头(带 + 新建钮)` → `收件箱置顶行(收件箱图标,点=浏览本机 recv_dir,self 设备)` → `常用条目行(名 + 成员色点串)`;位于「MTDSS设备」组上方。未登录面不变(常用区随登录后主界面出现)。
- 常用节点:点=主区混合视图;**不可树展开**(裁量 #5);右键菜单=打开/重命名/管理成员/删除常用。
- 拖放落点:常用节点挂 `data-drop-loc` fav 键——单成员=直传该成员;多成员=成员选择弹层(§5.3)。
- 新建=命名对话框(id 生成、空成员合法),侧栏「+」入口;弹层空态亦有同一新建入口(见 §5.3)。

### 5.2 主区混合视图

- Loc 扩展 `{kind:"fav", id}`(nav.ts 单源扩展);面包屑=常用名单段;「上一级」出常用视图即入普通设备层,历史后退可回常用页(浏览器式历史零改)。
- **拉取=客户端扇出、渐进渲染(裁量 #15)**:对每个成员并行 `list(device,path)`;**应答逐成员到达、逐次过合并-排序管线**(行渐进出现,不等齐——远端冷握手 25-35s 的现状下,等齐=一个慢成员拖死整页,否决);离线成员凭设备快照**立即**入「不可用桶」,列表失败成员超时后入桶(超时沿既有口径:iOS RpcGuard/withTimeout、桌面桥接侧同面,不另定值);状态栏计数随并入进度更新。
- 成员路径=远端设备路径原串;**本机成员以真实 device_id 入库即可本机短路**(引擎 is_self 同认 "self" 别名与真实 id,ctrl.rs:127-129),无特判。
- **混排(默认)**:一张列表按当前排序菜单(名称/日期/大小/种类)统一排,文件夹恒前,现 sortEntries 管线复用;同名跨源两行并存。
- **分组**:查看菜单加「混合|分组」段(Toolbar viewSections,仅常用视图显示;默认混合;localStorage 全局记,裁量 #4)。节=成员(节头=设备名+路径末段+色点;离线/不可用节头灰显+「离线」/「无法访问」标注,无条目);节序=成员添加序;节内=当前排序。分组模式下各节宿主各挂本成员 `data-drop-loc`(拖入=直传该成员,不走弹层)。
- **离线/不可用面(用户确认)**:混合模式=列表底部一行小字「N 个成员离线或不可用」(悬停列明细);分组模式=如上灰节头。
- 空常用(0 成员)=空态+引导「到设备浏览中添加目录」。
- 子目录钻取=navigate 普通 `{device,path}`,**混合只在第一层**(用户裁定回放确认)。
- **行动面对等(伞形;对等=与本平台普通目录的现实能力集对等)**:Windows 混合视图动作集=普通目录**现实集**逐字(打开/剪切/复制/粘贴/刷新/下载到本机/删除,ContextMenu 现集);**「转发」在 Windows 的兑现=拖到侧栏设备节点**(行可拖,既有跨设备 upload 语义照用)——本波**不新增** Windows 目标设备选择器组件(裁量 #13);**粘贴进常用视图=单成员直进/多成员成员选择弹层**(同拖放语义,弹层复用;分组模式节内粘贴=该成员,裁量 #14);**多选拖拽复制到本地=跨多设备文件汇聚**(按来源设备分组发 N 个传输,伞形钦点项);剪切跨来源多选禁用(移动语义不可跨源,裁量 #6);删除=既有回收站确认梯语义照旧(Windows 删除安全门=V3-4 面,本波不前拉)。
- 状态栏计数=合并条目数 + 离线/不可用成员数注记。
- **外部拖入(资源管理器→窗口)同受理(2026-11-05 用户裁定「接」)**:现状外部拖入仅普通目录视图安装 handler、目标恒当前目录(App.tsx onDragDropEvent);本波把常用视图与侧栏常用节点纳入——外部拖入在窗口级做 `[data-drop-loc]` 位置命中(elementFromPoint,复用内部拖拽既有命中机制),落点语义与内部拖拽**完全一致**(单成员直进/多成员弹层;分组模式节宿主=该成员);modalBusy 门与拖拽管线既有坑位(dragDropEnabled 抑制 HTML5 DnD 等)随接线核验。普通目录的外部拖入行为逐字节不变(桌面零回归)。

### 5.3 添加成员与成员选择弹层

- 入口:设备浏览主区**目录行右键「添加到常用目录」**;多选(纯目录)同项可用,含文件或混合选择时禁用;侧栏树节点不加右键(裁量 #12)。
- 弹层=常用清单(名 + 成员数);点选即加(去重静默)+ toast;空态=「新建常用目录」入口(同命名对话框)。
- 多成员常用作为**拖放目标**时同一弹层(行=设备名+路径+色点,选即传)——伞形「弹层选存到哪个成员」。

### 5.4 管理屏(右键「管理成员」)

- 对话框:重命名(即时生效)、成员列表(色点+设备名+路径+移除钮)、删除常用(确认弹窗,文案明示「只解除链接,不删除文件」)。成员设备已不在快照=标注「设备已移除」,可清。

### 5.5 桥与数据层

- ipc_bridge 增 `kv_get/kv_put` Tauri 命令 + `daemon://data_changed` 事件转发(lib.rs generate_handler 增列)。
- `lib/favorites.ts`:schema 解析/规范化/去重/v 守卫、色分配纯函数、混合合并-排序-分组纯函数、store(拉取缓存+事件重拉+编辑整单 put)——vitest 全覆盖;组件测试沿 ApprovalModal.test.tsx 先例。
- i18n zh.json 增条目;App.css 增色点/节头/收件箱行样式。

## 6. iOS 落地

- **MTDSSKit**:Contract.swift IpcEvent 增 `dataChanged(key)` case + parse(golden 同步);FavoritesStore(kv 封装 + 事件重拉 + 同 §3 schema/算法);AppModel 挂 favorites。MTDSSKitTests:store 解析/编辑/事件重拉、parse data_changed、混合 VM 纯函数。
- **常用标签(FavoritesView)**:收件箱根行**下方**增常用条目行(名+成员色点串);点=推混合页;左滑=[管理, 删除常用];管理页=重命名/成员列表可移除/删除常用(确认,同款「只解除链接」文案)。
- **混合页(新 FavoritesMixedView)**:混排默认、分组可选(「···」菜单「显示方式」,AppStorage 全局记);排序菜单同浏览页;行首来源色点;子目录点按=推该设备普通浏览页;离线/不可用/空态/同名并存语义同 §5.2;**动作=下载到收件箱/发送到设备(DevicePickerSheet)/删除(远端门 allowRemoteDelete 照旧;本机成员行=本地删除)**;选择模式复用既有骨架;无「放入」。
- **添加成员入口**:设备浏览页目录行滑删动作集 +「常用」(仅目录行);选择模式纯目录选择时底栏 +「常用」;弹层同 §5.3(选常用/空态新建)。
- **本机目录作成员**:渲染面支持(self 别名);iOS 侧不设本机目录添加入口(本机常用面已有收件箱置顶)——裁量 #7。

## 7. daemon / IPC / 服务端增量

- hub.rs `data_changed` → `event.data_changed {key}` 事件面(镜像 devices_changed 先例,hub.rs:585 留痕位点亮);golden +`event.data_changed.json`,REGEN 三端契约(Kotlin/Swift 契约测试同步;安卓 V3-4 才有 UI 消费)。
- `kv.get/kv.put` IPC 已在(V3-1),**零新 op**;list/delete/transfer 零改;**服务端零改**;混合拉取=客户端 N 路 list 扇出,引擎无感。

## 8. 测试策略

- **Rust**:hub data_changed→事件帧单测;golden REGEN 全绿;kv 面既有测试零回归。
- **TS(vitest)**:favorites schema 规范化/去重/v 守卫;色分配(排序位次/回绕);混合合并-排序-分组纯函数;Loc fav 扩展导航;picker/管理屏组件测试。
- **Swift**:FavoritesStore(解析/编辑/LWW 重拉/v 守卫);Contract parse event.data_changed;混合 VM 合并排序分组离线;golden 契约测试同步。
- **e2e/冒烟**:双 daemon 同账号 kv 编辑互推(data_changed→事件→对端重拉);三成员混合视图(一离线)扇出合并;PC 拖入多成员弹层(内部指针拖 + 资源管理器外部拖两路);粘贴进多成员常用;真机项进 `docs/smoke-v3p3.md`(沿 V3-1/V3-2 手册体例)。

## 9. 兼容性

- event.data_changed=新事件面:旧端「未知事件忽略」语义(Contract 未知 method→nil 先例)零影响;**本波无三端同批发版约束**(纯消费端增量,服务端/daemon 先升无害)。
- 安卓:golden 契约测试随 REGEN 更新即绿,V3-4 落地 UI。
- 桌面零回归:既有侧栏/浏览/拖放行为除明示新增项外逐字节不变(伞形反目标)。

## 10. 记录人裁量清单(交审一并过目)

1. kv 键名 `favorites`;schema v1 形态;id=16 hex 客户端随机;成员只存 device_id+path;
2. 整单 LWW 无 CAS,并发编辑后写者胜,接受;
3. 色板 6 色如 §4 表;device_id 排序位次分配;>6 回绕;设备增删移色接受;**3b. 行级着色=行首色点轻方案,不整行铺色**;
4. 混合|分组=全局偏好(localStorage/AppStorage),非每常用记忆;默认混合;
5. 常用节点不可树展开;「上一级」出常用即设备层;
6. 剪切跨来源多选禁用;复制/下载/发送跨来源=按来源分组 N 个传输;
7. iOS 不设本机目录添加入口(渲染面仍支持本机成员);
8. 空成员合法;去重静默;收件箱可作成员;
9. 无分页/截断(与普通浏览同现状,单成员大目录不劣化);
10. 侧栏常用区仅登录后出现;
11. 条目序/节序=创建/添加序,无手动排序;
12. 成员添加只在主区右键/多选(侧栏树节点不加右键菜单);弹层空态含新建入口;
13. 混合视图动作集=本平台普通目录现实集(Windows 无「发送」菜单项,转发=拖到设备节点;**不新增 Windows 目标设备选择器**——伞形「转发」在 Windows 端以既有拖放面兑现);
14. 粘贴进常用视图=单成员直进/多成员弹层,与拖放同语义;
15. 扇出=渐进渲染(逐成员并入合并-排序管线),离线凭快照即时入桶、失败超时入桶(沿既有超时口径),**不等齐**;
16. 编辑失败=put 成功才落本地(服务端为准),失败 toast 保旧值;信令 WS 离线但会话在允许编辑(kv op 走 REST)。

## 11. 留给实施计划的

- Windows App.tsx 的 Loc 扩展/拖放/ContextMenu 接线切面;FavoritesStore 生命周期(登出清理/重登录重拉);
- iOS MixedViewModel 与 BrowseViewModel 的复用边界(骨架新建 vs 扩展);FavoritesView 根列表扩展切面;
- 弹层/管理屏的组件复用选型(ConfirmDialog/菜单件既有面);
- vitest/Swift 测试文件划分;e2e 与冒烟清单细化;DEBUG 截图钩子(MTDSS_DEBUG_* 先例)。

## 12. 实施回填(2026-11-06)

- **波概览**:常用目录系统全量落地——daemon `event.data_changed` 事件面+golden 三端契约、Windows 桥 kv 命令/事件转发、TS 模型/store/混合纯模型、Windows UI 全消费面(侧栏常用区/混合·分组视图/拖放三路/动作对等/管理屏)、MTDSSKit FavoritesStore+MixedViewModel、iOS UI(常用标签根列表/混合页/添加成员入口/管理屏);服务端与 proto 零改。分支 `v3p3` = main@13c9347 + T1-T18 共 20 commit(13c9347..90f7980,含 1 枚 lockfile chore)+ T19 收尾批与本回填 2 commit;自动化面全绿(vitest 375、swift test 全量、cargo 全 workspace、gradlew 契约)。
- **偏差要点(均已在任务档入案)**:
  - 模态占线(modalBusy)槽位按实现正扩展:favName/favPick/favMemberPick/favMenu/favManage/favDelFor 六新槽(§5 既有占线门机制的合同内扩展,T8/T13);
  - fav 位行拖出窗口:实施计划内静默忽略(T12 遗留),T19 收尾批补齐为**可见化 fan-out**(解析一次落点目录,按来源组逐组 download,本机组跳过,downloadMixed 同款冲突停语义)——§5.2 拖放语义就此完备;
  - iOS 新建常用后的成员直加采用**认领取尾部 id**(依赖 T14 创建序不变量:create 恒尾部追加且 put 成功才落 @Published;控制者裁定,不用 prevIds 差集,T18);
  - 多源分组传输的冲突推进=T11 评审修复引入的 `runGroupsWithConflictPause` 驱动器(park 即停+余组计数 toast),§5.2「按来源分组 N 个传输」在此语义上推进;
  - iOS 端不设本机目录添加入口(裁量 #7)由 App 接线面收口,Kit 函数面保持完备并测试钉(T18);iOS 混合页钻取的远端浏览无「常用」入口=已知边界(冒烟手册 §F5 记录项);
  - 其余 deferred minors(纯测试锚/注释/视觉裁量级)逐任务记档于 SDD ledger,不进验收线。
