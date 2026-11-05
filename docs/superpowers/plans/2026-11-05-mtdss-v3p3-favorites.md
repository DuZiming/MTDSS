# MTDSS V3-3 常用目录系统 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 spec(`docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md`)的账号级常用目录清单落成:Windows 侧栏常用区 + 混合/分组视图 + 拖放/粘贴/动作对等 + iOS「常用」标签扩展 + 终端标志色 + 收件箱置顶,daemon 事件面与 golden 契约同步。

**Architecture:** 数据 = 单 kv 键 `favorites` 的整单 JSON(整单 LWW,put 成功才落本地);混合视图 = 客户端对成员并行 `list` 扇出、渐进合并渲染;行身份 = `device\0path` 复合键(同路径跨源不串选);颜色 = device_id 排序位次 → 固定 6 色板,两端同算法。服务端零改,proto 零改。

**Tech Stack:** Rust(daemon/Tauri 桥)、React 19 + TypeScript + vitest(ui/)、SwiftUI + MTDSSKit(swift test 于 Mac mini)、Kotlin 契约测试(android/)。

**Spec:** `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md`(main@0bc379e 已入库;§10 裁量清单与 §5/§6 是本计划的语义源)

## Global Constraints

- 服务端与 proto **零改**;IPC golden 变更走 REGEN(`$env:REGEN="1"; cargo test -p mtdss-daemon --test contract_golden`,幂等),Kotlin/Swift 契约测试同批同步。
- kv 键名恒 `favorites`;schema v1 见 spec §3.1(成员只存 device_id+path 原串);`v>1` → 只读、绝不回写。
- 色板 6 色值 verbatim:`#14B8A6 #06B6D4 #8B5CF6 #D946EF #EC4899 #64748B`;分配=设备快照按 device_id 字典序位次 mod 6。
- 默认混合视图;混合|分组=全局偏好(Windows localStorage / iOS AppStorage),不按条目记忆。
- 混合只在第一层;子目录钻取=普通单设备浏览。剪切跨来源多选禁用;跨来源多选的下载/复制/拖放=按来源设备分组 N 个传输。
- 删除常用/移除成员=只解除链接,不碰远端文件(确认文案明示)。
- 扇出=渐进渲染(逐成员并入,不等齐);离线成员凭设备快照即时入桶(不发 list);list 失败成员超时入桶(超时沿既有口径)。
- 编辑失败语义:kv.put 成功才落本地;失败 toast 保旧值;编辑前置 kv.get 取最新。
- 桌面零回归:普通目录既有行为(浏览/拖放/粘贴/菜单)除明示新增项外逐字节不变;未登录常用区不出现。
- UI 文案全中文(zh.json / Swift 字面量);代码注释沿仓库房风(中文 /// 文档注释)。
- iOS 测试在 Mac mini 执行(SSH;钥匙串 unlock 见 docs/runbook-v3.md 与项目记忆);Windows 本机跑 cargo / vitest / tsc / gradlew。

## Review Focus

1. **v 守卫回写洞**:旧端拿到 `v>1` 的文档后任何编辑路径都不得 put(否则覆盖新版数据)——T3 parse 测试 + T4 store put 前置 + T14 Swift store 同断言。
2. **同路径跨源串选**:混合视图两条 `path` 相同的行,选择/右键/删除必须按 `device\0path` 复合键区分——T6 纯函数 + T9 SSR + T15 VM 测试各钉一枚。
3. **离线判定与 list 失败分桶竞态**:快照离线→立即 offline 桶且**不发 list**;快照在线但 list 拒/超时→failed 桶——T6 `offlineMembers` 与 `memberStateUpsert` 测试钉死两桶入口。
4. **外部拖入坐标与占线门**:Tauri drop 的 position 是物理像素,elementFromPoint 要除以 devicePixelRatio;冲突/确认弹窗开着时外部拖入不得受理(否则顶掉待决批)——T12 步骤内双钉。
5. **put 失败/编辑前置竞态**:编辑必先 kv.get 最新再改再 put;put 失败本地保旧值——T4(mock 两次 get 返回不同文档)与 T14(FakeDaemon 序列)。

---

### Task 1: daemon `event.data_changed` 事件面 + golden + 三端契约同步

**Files:**
- Modify: `crates/daemon/src/hub.rs:57`(EV 常量区)、`hub.rs:585`(data_changed 留痕位)
- Modify: `crates/daemon/tests/contract_golden.rs`(fixture 区 ~1179 旁)
- Modify: `crates/daemon/tests/login_v3p1.rs:670-725`(KV 透传既有测试)
- Modify: `ios/package/Sources/MTDSSKit/Ipc/Contract.swift:42-49`(Ops)、`Contract.swift:563-655`(IpcEvent)
- Modify: `android/app/src/test/java/com/mtdss/android/ipc/ContractGoldenTest.kt`
- Create: `crates/ipc/contract/event.data_changed.json`(REGEN 生成)

**Interfaces:**
- Produces: IPC 事件 `event.data_changed`,params `{"key": "<string>"}`;Swift `IpcEvent.dataChanged(key: String)` + `Ops.eventDataChanged = "event.data_changed"`(T15 AppModel 订阅消费)。

- [x] **Step 1: 写失败测试(login_v3p1.rs KV 透传测试扩展)**

既有测试(670 行区)已做「B 写 → A 读同值;data_changed 通告后 A 连接保持」。扩展:put 成功后断言 A 的事件流收到 `event.data_changed` 帧、`params.key == "favorites"`。事件收集方式沿该测试既有的事件客户端模式(先读测试上下文再仿写)。

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test -p mtdss-daemon --test login_v3p1`
Expected: FAIL(收到的是无事件/留痕)。

- [x] **Step 3: 实现 hub.rs 路由**

`const EV_DATA_CHANGED: &str = "event.data_changed";`(EV_DEVICES_CHANGED 旁);`"data_changed" => { events.send(EventFrame { method: EV_DATA_CHANGED.into(), params: json!({ "key": data_key }) }); }`(镜像 devices_changed 臂;注释注明 spec §7)。

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test -p mtdss-daemon --test login_v3p1`
Expected: PASS。

- [x] **Step 5: golden fixture + REGEN**

contract_golden.rs 事件 fixture 区(`event.devices_changed.json` 旁)insert `("event.data_changed.json".into(), event_frame("event.data_changed", json!({ "key": "favorites" })))`;PowerShell:`$env:REGEN="1"; cargo test -p mtdss-daemon --test contract_golden`,去 env 再跑一次确认逐字节绿。

- [x] **Step 6: Swift 契约同步 + 测试**

Contract.swift:`Ops.eventDataChanged = "event.data_changed"`;`IpcEvent` 增 `case dataChanged(key: String)`;parse 增 case(`params.req("key").asString()`,未知/畸形 → nil 同房规)。ContractGoldenTests 若按目录枚举 golden 则新文件自动入列,否则补条目。
Run(Mac mini): `cd ios/package && swift test --filter ContractGoldenTests`
Expected: PASS。

- [x] **Step 7: Kotlin 契约同步 + 测试**

ContractGoldenTest.kt 以 `devices_changed` 为锚镜像补 `event.data_changed.json` 断言(该文件的既有列举方式照抄)。
Run: `cd android && ./gradlew.bat :app:testDebugUnitTest --tests "com.mtdss.android.ipc.ContractGoldenTest"`(SDK 缺则记 CI 兜底)
Expected: PASS。

- [x] **Step 8: Commit**

```bash
git add crates/daemon/src/hub.rs crates/daemon/tests/contract_golden.rs crates/daemon/tests/login_v3p1.rs crates/ipc/contract/event.data_changed.json ios/package/Sources/MTDSSKit/Ipc/Contract.swift android/app/src/test/java/com/mtdss/android/ipc/ContractGoldenTest.kt
git commit -m "feat(daemon): data_changed 通告点亮为 event.data_changed 事件面(V3-3 契约:golden REGEN+Swift/Kotlin 同步)"
```

### Task 2: Tauri 桥 kv_get/kv_put 命令 + daemon://data_changed 转发

**Files:**
- Modify: `ui/src-tauri/src/ipc_bridge.rs`(命令区 ~738 approval_respond 旁、event_action 表 ~234、测试对列表 ~920)
- Modify: `ui/src-tauri/src/lib.rs:78-113`(generate_handler)

**Interfaces:**
- Produces: Tauri 命令 `kv_get(state, key: String) -> Result<Value, String>`、`kv_put(state, key: String, value: String) -> Result<Value, String>`;窗口事件 `daemon://data_changed`(payload=`{"key":...}`)。T4 的 TS 封装消费。

- [x] **Step 1: 写失败测试**

ipc_bridge.rs 测试区(920 行旁事件对列表)补 `("event.data_changed", "daemon://data_changed")` 断言;命令注册测试若有(grep `generate_handler` 测试)则补两命令。

- [x] **Step 2: 跑测试确认失败**

Run: `cargo test --manifest-path ui/src-tauri/Cargo.toml`(桥单测所在包;若无独立测试模块,以编译 + event_action 表测试为准)
Expected: FAIL(无该映射)。

- [x] **Step 3: 实现**

event_action 表增 `"event.data_changed" => EventAction::Forward("daemon://data_changed")`;命令两枚照 approval_respond 模板:`rpc(&state, "kv.get", json!({"key": key}), RPC_TIMEOUT)` / `"kv.put"` + `json!({"key": key, "value": value})`;lib.rs generate_handler 增 `ipc_bridge::kv_get, ipc_bridge::kv_put`。错误串化机制不动(daemon 的 not_found 带 `data.kind`,TS 侧 toRpcError 已能还原)。

- [x] **Step 4: 跑测试确认通过**

Run: `cargo test --manifest-path ui/src-tauri/Cargo.toml && cargo check -p mtdss-ui`(包名以 Cargo.toml 为准)
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src-tauri/src/ipc_bridge.rs ui/src-tauri/src/lib.rs
git commit -m "feat(ui-bridge): kv_get/kv_put 命令透传 + daemon://data_changed 事件转发(V3-3)"
```

### Task 3: TS favorites schema 模型与色板(纯函数)

**Files:**
- Create: `ui/src/lib/favorites.ts`
- Test: `ui/src/lib/favorites.test.ts`

**Interfaces:**
- Produces(T4/T6/T8 消费):

```ts
export const FAVORITES_KV_KEY = "favorites";
export interface FavMember { device: string; path: string }
export interface Favorite { id: string; name: string; created_at: number; members: FavMember[] }
export interface FavoritesDoc { v: 1; updated_at: number; favorites: Favorite[] }
export const EMPTY_FAVS: FavoritesDoc;
export interface FavsParse { doc: FavoritesDoc; readOnly: boolean }
export function parseFavorites(raw: string | null): FavsParse | null; // null=无键/空串
export function newFavId(): string; // crypto 16 hex
export function createFavorite(doc: FavoritesDoc, name: string, now: number): FavoritesDoc;
export function renameFavorite(doc: FavoritesDoc, id: string, name: string): FavoritesDoc;
export function removeFavorite(doc: FavoritesDoc, id: string): FavoritesDoc;
export function addMembers(doc: FavoritesDoc, id: string, members: FavMember[]): { doc: FavoritesDoc; added: number }; // 同(device,path)去重
export function removeMember(doc: FavoritesDoc, id: string, member: FavMember): FavoritesDoc;
export function memberKey(m: FavMember): string; // `${device}\u0000${path}`
export const DEVICE_COLOR_HEX: readonly string[];
export function deviceColorMap(devices: { device_id: string }[]): Map<string, string>;
```

- [x] **Step 1: 写失败测试**

覆盖:parse null/合法 v1 round/`v:2` → readOnly=true;createFavorite 尾部追加(创建序)且 id 唯一;addMembers 去重(added 计数)与跨常用重复允许;removeMember/removeFavorite 不碰其他条目;memberKey;deviceColorMap 按 device_id 字典序位次取色(构造 3 台断言三色)、>6 台回绕(第 7 台=第 1 色)。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/lib/favorites.test.ts`
Expected: FAIL(模块不存在)。

- [x] **Step 3: 实现 favorites.ts**

模块头 `///` 注释引 spec §3/§4;`v>1` 或解码失败但对象含更高 v → readOnly(结构已知字段尽力解析);decode 失败且无 v → readOnly + EMPTY。newFavId 用 `crypto.getRandomValues` 8 字节 hex。纯函数全部返回新对象(房房 style 与 nav.ts 同)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/lib/favorites.test.ts`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/lib/favorites.ts ui/src/lib/favorites.test.ts
git commit -m "feat(ui): favorites schema 模型+终端标志色板(纯函数,V3-3 §3/§4)"
```

### Task 4: TS kv 封装与 useFavorites store

**Files:**
- Modify: `ui/src/lib/daemon.ts`(kvGet/kvPut/listenDataChanged,模块头注释清单同步)
- Create: `ui/src/lib/favstore.ts`
- Test: `ui/src/lib/favstore.test.ts`

**Interfaces:**
- Consumes: T2 桥命令、T3 模型。
- Produces(T8 起 UI 消费):

```ts
// daemon.ts
export async function kvGet(key: string): Promise<string | null>; // not_found(kind)→null
export async function kvPut(key: string, value: string): Promise<void>;
export function listenDataChanged(cb: (key: string) => void): Promise<UnlistenFn>;
// favstore.ts
export interface FavStoreState { doc: FavoritesDoc; readOnly: boolean; ready: boolean; busy: boolean }
export function useFavorites(loggedIn: boolean): {
  state: FavStoreState;
  refresh(): Promise<void>;
  createFav(name: string): Promise<boolean>;
  renameFav(id: string, name: string): Promise<boolean>;
  deleteFav(id: string): Promise<boolean>;
  addMembers(id: string, members: FavMember[]): Promise<number>;
  removeMember(id: string, member: FavMember): Promise<boolean>;
};
```

- [x] **Step 1: 写失败测试**

`vi.mock("./daemon")`:①登录翻转→kvGet 调用;②kvGet 返回 null→EMPTY ready;③`v:2`→readOnly 且 createFav 返回 false 且 **kvPut 零调用**(Review Focus #1);④createFav:mock kvGet 第一次返回他端新文档→断言 put 的是「最新+新条目」(Review Focus #5);⑤kvPut reject→state 保旧值(Review Focus #5);⑥listenDataChanged("favorites")→refresh;key 不符→不刷。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/lib/favstore.test.ts`
Expected: FAIL。

- [x] **Step 3: 实现**

daemon.ts:kvGet catch→`kind(toRpcError(e))==="not_found" ? null : throw`;listenDataChanged 照 listenDevices 模式(`listen<{key:string}>("daemon://data_changed")`)。favstore.ts:useState+FavStoreState;编辑 op 统一私函 `mutate(fn:(doc)=>FavoritesDoc|null)`=kvGet→parse→readOnly 拒→fn→kvPut→成功才 setState;事件订阅 useEffect(仅 loggedIn)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/lib/favstore.test.ts && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/lib/daemon.ts ui/src/lib/favstore.ts ui/src/lib/favstore.test.ts
git commit -m "feat(ui): kv 封装+useFavorites store(整单 LWW·put 成功才落·data_changed 重拉)"
```

### Task 5: Loc 扩展 fav 位

**Files:**
- Modify: `ui/src/lib/nav.ts:15`
- Modify: `ui/src/App.tsx:225`(sameLoc)
- Test: `ui/src/lib/nav.test.ts`(既有文件追加)

**Interfaces:**
- Produces: `export type Loc = { kind: "devices" } | { kind: "dir"; device: string; path: string } | { kind: "fav"; id: string }`;`parentLoc` fav→devices;App.sameLoc fav 比较 id。

- [x] **Step 1: 写失败测试**

nav.test.ts:parentLoc({fav,id:"f1"})==={devices};navGo/Back/Forward 携 fav 位无损。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/lib/nav.test.ts`
Expected: FAIL。

- [x] **Step 3: 实现**

nav.ts 类型扩展 + parentLoc 分支(注释:常用视图上级=设备层,spec 裁量 #5);App.sameLoc 增 `a.kind==="fav" && b.kind==="fav" && a.id===b.id` 分支。tsc 全绿(fav 位尚无生产者/消费者,仅类型完备)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/lib/nav.test.ts && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/lib/nav.ts ui/src/lib/nav.test.ts ui/src/App.tsx
git commit -m "feat(ui): Loc 扩展 fav 导航位(上级=设备层,V3-3 §5.2)"
```

### Task 6: 混合视图纯模型(渐进合并/分组/离线桶/跨源分组)

**Files:**
- Create: `ui/src/lib/mixedview.ts`
- Test: `ui/src/lib/mixedview.test.ts`

**Interfaces:**
- Consumes: T3 FavMember、browse.ts FsEntry/SortState/sortEntries、collate Cmp。
- Produces(T9/T10/T11/T12 消费):

```ts
export interface MixedRow { entry: FsEntry; device: string; key: string } // key=`${device}\u0000${entry.path}`
export type MemberStatus = "pending" | "ok" | "offline" | "failed";
export interface MemberState { member: FavMember; status: MemberStatus; entries: FsEntry[] }
export function offlineMembers(members: FavMember[], online: (d: string) => boolean): MemberState[];
export function memberStateUpsert(states: MemberState[], member: FavMember, status: MemberStatus, entries: FsEntry[]): MemberState[];
export function mixedRows(states: MemberState[], sort: SortState, cmp: Cmp): MixedRow[];
export interface GroupedSection { member: FavMember; state: MemberState; rows: MixedRow[] }
export function groupedSections(states: MemberState[], sort: SortState, cmp: Cmp): GroupedSection[];
export function unavailableCount(states: MemberState[]): number; // offline+failed
export interface SourceSel { device: string; entries: FsEntry[] }
export function groupRowsByDevice(rows: MixedRow[], selected: ReadonlySet<string>): SourceSel[];
```

- [x] **Step 1: 写失败测试**

①渐进:两成员先到 A 后到 B,mixedRows 两次调用结果各自正确(排序稳定、目录恒前);②同路径跨源(Review Focus #2):两成员各含 `path` 相同文件→两行两 key;③offlineMembers:快照离线→offline(不发 list 的契约由调用方保证,函数只按 online 判);memberStateUpsert failed 桶(Review Focus #3);④groupedSections 节序=成员添加序、离线节 entries 空;⑤unavailableCount;⑥groupRowsByDevice:3 行 2 设备选中→2 组,组内保持渲染序。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/lib/mixedview.test.ts`
Expected: FAIL。

- [x] **Step 3: 实现**

mixedRows = states.flatMap(ok 行)→复用 sortEntries 排 FsEntry,再映射回 MixedRow(排序稳定保成员序 tie);模块注释引 spec §5.2 渐进渲染裁定。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/lib/mixedview.test.ts`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/lib/mixedview.ts ui/src/lib/mixedview.test.ts
git commit -m "feat(ui): 混合视图纯模型(渐进合并/分组节/离线失败桶/跨源分组)"
```

### Task 7: 三视图 rowKey + sourceColor 可选面

**Files:**
- Modify: `ui/src/components/FileList.tsx`、`FileTiles.tsx`、`FileDetails.tsx`(DirViewProps 各自消费)
- Test: `ui/src/components/FileList.test.tsx`(新)

**Interfaces:**
- Produces: `DirViewProps` 增可选 `rowKey?: (e: FsEntry) => string`(默认 `e.path`;React key、`data-row-path`、selected/rangeSet/cutPaths/dragSourcePaths 判定全走它)与 `sourceColor?: (e: FsEntry) => string | undefined`(非 undefined→行首 `<span class="src-dot" style={{background}}>`,aria-hidden)。

- [x] **Step 1: 写失败测试(SSR 冒烟,沿 ApprovalModal.test.tsx 手法)**

renderToStaticMarkup 渲染 FileList:rows 两条同名同 path 不同 device,rowKey=复合键→输出含两个 `data-row-loc` 不同的行(以 data-row-path 断言);sourceColor→输出含 `src-dot` span 与背景色值;不传两 prop→输出与既有形态零 diff(零回归锚)。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/FileList.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

三组件同构接线(FileDetails 色点置于行首图标前;排序/表头逻辑零动)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/FileList.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/components/FileList.tsx ui/src/components/FileTiles.tsx ui/src/components/FileDetails.tsx ui/src/components/FileList.test.tsx
git commit -m "feat(ui): 三视图可选 rowKey+sourceColor(混合视图复用面,缺省零改动)"
```

### Task 8: 侧栏常用区 + 新建对话框 + App 接线

**Files:**
- Modify: `ui/src/components/Sidebar.tsx`(灰占位 229-235 点亮)
- Create: `ui/src/components/FavNameDialog.tsx`
- Modify: `ui/src/App.tsx`(useFavorites 接线、fav 导航、收件箱行)
- Modify: `ui/src/i18n/zh.json`、`ui/src/App.css`
- Test: `ui/src/components/FavNameDialog.test.tsx`

**Interfaces:**
- Consumes: T3/T4/T5。
- Produces: Sidebar 新 props `favorites: Favorite[]; colors: Map<string,string>; deviceName(id:string):string; inboxPath: string|null; onOpenFav(id:string):void; onOpenInbox():void; onNewFav():void; onManageFav(id:string):void`;常用行 `data-drop-loc=\`fav\u0000${id}\``;收件箱行 `data-drop-loc=\`self\ u0000…\`` 形(`${selfDeviceId}\u0000${inboxPath}`,T12 消费)。FavNameDialog props `{ open: boolean; initial?: string; busy: boolean; onSubmit(name: string): void; onCancel(): void }`(T13 重命名复用)。

- [x] **Step 1: 写失败测试**

FavNameDialog SSR:open=false 不渲染;open 渲染标题/输入(初始值)/确认取消词面全为 zh 键值。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/FavNameDialog.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

Sidebar:占位换分组(标头「常用目录」+`+` 钮;收件箱行=收件箱图标+`收件箱`;常用行=名+成员色点串,点=onOpenFav,右键=onManageFav,拖放 data-drop-loc 如上;未登录由 App 不传 favorites 区(App 条件渲染——loggedIn 才渲染常用区,全局约束)。App:`useFavorites(loggedIn)`;`onOpenFav` → `navigate({kind:"fav", id})`;`onOpenInbox` → `navigate({kind:"dir", device:selfDeviceId, path:status.recv_dir})`(selfDeviceId/recv_dir 缺→no-op);`onNewFav` 开 FavNameDialog→createFav→toast;colors=deviceColorMap(devices)。zh.json 增 sidebar.favoritesNew/inbox/manage 等键;App.css 增 .src-dot/.fav-dots/.tree-node 复用样式。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/FavNameDialog.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/components/Sidebar.tsx ui/src/components/FavNameDialog.tsx ui/src/components/FavNameDialog.test.tsx ui/src/App.tsx ui/src/i18n/zh.json ui/src/App.css
git commit -m "feat(ui): 侧栏常用区点亮(收件箱置顶+常用清单+新建)+fav 导航接线"
```

### Task 9: FavView 混合视图主体(渐进扇出/复合键选择/离线面)

**Files:**
- Create: `ui/src/lib/favfetch.ts`、`ui/src/components/FavView.tsx`
- Modify: `ui/src/App.tsx`(fav 位渲染分支、StatusBar、状态栏注记)
- Modify: `ui/src/i18n/zh.json`、`ui/src/App.css`
- Test: `ui/src/lib/favfetch.test.ts`、`ui/src/components/FavView.test.tsx`

**Interfaces:**
- Consumes: T6/T7;daemonList。
- Produces:

```ts
// favfetch.ts
export function useFavListing(fav: Favorite, devices: DeviceInfo[], reloadTick: number): {
  states: MemberState[]; loading: boolean;
};
// FavView.tsx props
{
  fav: Favorite; devices: DeviceInfo[]; colors: Map<string,string>;
  viewMode: ViewMode; sortState: SortState;
  selected: ReadonlySet<string>; onSelected(next: ReadonlySet<string>): void;
  searchQ: string;
  onOpenDir(device: string, path: string): void;
  onRowContextMenu(row: MixedRow, x: number, y: number): void;
  onBlankContextMenu(x: number, y: number): void;
  dropHoverKey: string | null;
}
```

- [x] **Step 1: 写失败测试**

favfetch.test(vi.mock daemonList):①快照离线成员→初始即 offline、**daemonList 不被以该成员调用**(Review Focus #3);②在线成员应答→ok 段并入;③reject→failed;④fav 变更(导航换常用)→旧应答丢弃(seq 守卫)。FavView SSR:两成员一离线→离线文案行/计数注记出现;空常用(0 成员)→空态引导文案。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/lib/favfetch.test.ts src/components/FavView.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

useFavListing:初始 states=offlineMembers(members, deviceOnline);对非 offline 成员并行 daemonList,每应答 memberStateUpsert(seq=fav.id+reloadTick 守卫,房风同 App 655-686)。FavView:mixedRows→filterEntries(searchQ)→经三视图(现有 viewMode 分支)传 rowKey=复合键、sourceColor=colors.get(device);行 click/双击/右键经回调上抛(双击目录→onOpenDir(row.device, entry.path)——钻取=普通 dir 位,spec「混合只在第一层」);底部离线/不可用注记行(`N 个成员离线或不可用`,悬停 title 列明细);宿主 data-drop-loc=`fav\u0000${fav.id}`(T12 消费)。App:fav 位渲染 FavView(注意:dirProps 分支不动);StatusBar 增可选 `note?: string` prop(itemCount=mixed 行数)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/lib/favfetch.test.ts src/components/FavView.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/lib/favfetch.ts ui/src/components/FavView.tsx ui/src/lib/favfetch.test.ts ui/src/components/FavView.test.tsx ui/src/App.tsx ui/src/components/StatusBar.tsx ui/src/i18n/zh.json ui/src/App.css
git commit -m "feat(ui): 混合视图主体(渐进扇出/复合键/离线不可用面/空态引导)"
```

### Task 10: 查看「混合|分组」段与分组渲染

**Files:**
- Modify: `ui/src/components/Toolbar.tsx`(viewSections ~203)
- Modify: `ui/src/components/FavView.tsx`(分组渲染)
- Modify: `ui/src/App.tsx`(viewOrg 状态+localStorage)
- Test: `ui/src/components/Toolbar.test.tsx`(新)

**Interfaces:**
- Produces: Toolbar 增 props `orgToggleVisible: boolean; viewOrg: "mixed" | "grouped"; onViewOrg(org: "mixed" | "grouped"): void`;localStorage 键 `"favViewOrg"`(load/save 小函数在 Toolbar.tsx 导出供 App 初始化)。

- [x] **Step 1: 写失败测试**

Toolbar SSR:orgToggleVisible=false→无「混合/分组」段(普通目录零回归锚);true→段在、两项 radio 词面 zh。loadFavViewOrg():坏值→"mixed"。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/Toolbar.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

viewSections 条件追加段(MenuSection 既有结构);App `const [viewOrg, setViewOrg] = useState(loadFavViewOrg)`+persist;FavView viewOrg==="grouped" 时按 groupedSections 渲染:节头行(色点+设备名+路径末段;offline/failed→灰显+`离线`/`无法访问` 标注,无条目)、节内行 mixed 渲染同构、在线节宿主 data-drop-loc=`${member.device}\u0000${member.path}`;节排序=成员添加序。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/Toolbar.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/components/Toolbar.tsx ui/src/components/Toolbar.test.tsx ui/src/components/FavView.tsx ui/src/App.tsx
git commit -m "feat(ui): 查看菜单混合|分组段(默认混合·全局偏好)+分组节渲染"
```

### Task 11: 动作对等 + 「添加到常用目录」

**Files:**
- Modify: `ui/src/components/ContextMenu.tsx`(增项)、`ui/src/App.tsx`(下载/删除/复制剪切门、菜单装配、FavPickerDialog 接线)
- Create: `ui/src/components/FavPickerDialog.tsx`
- Modify: `ui/src/i18n/zh.json`
- Test: `ui/src/components/FavPickerDialog.test.tsx`

**Interfaces:**
- Consumes: T4 addMembers、T6 groupRowsByDevice。
- Produces: ContextMenu 增 props `canAddFav: boolean; onAddFav(): void`(菜单项「添加到常用目录」);App 内 `downloadMixed(sel: ReadonlySet<string>): void`、`deleteMixed(sel): void`、`copyCutMixed(sel, cut): void`(T12 复用);FavPickerDialog props `{ open; favorites: Favorite[]; onPick(id: string): void; onNew(): void; onClose(): void }`。

- [x] **Step 1: 写失败测试**

FavPickerDialog SSR:清单渲染名+成员数;空清单→空态文案+「新建常用目录」钮。App 纯逻辑(groupRowsByDevice 已在 T6 测):无新纯函数则免。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/FavPickerDialog.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

①目录浏览:App 计算 canAddFav=dir 位且(右键目标是目录 或 多选全目录且非空);菜单项→setFavPickFor({device, paths:[…]})→FavPickerDialog→addMembers→toast(`已加入 {名}`)。②FavView 右键(onRowContextMenu):App 以 MixedRow 装配菜单——打开(目录,单选)/下载到本机…/复制/剪切(仅单源组可用,跨源禁用)/删除;blank 菜单=粘贴(T12)/刷新。③downloadMixed:pickFolder 一次→groupRowsByDevice 逐组:本机组跳过、远端组 runTransfer(download, dir=选中目录)(伞形「跨多设备汇聚」)。④deleteMixed:ConfirmDialog 复用(confirmDel 形态扩为 `{groups:{device,paths}[]}` 常规删除每组 daemonDelete(force=false);RECYCLE_UNAVAILABLE force 批逐组重发——confirmForceDelete 扩多组)。⑤Toolbar opsEnabled/Del 键/Ctrl+X/C 在 fav 位路由到上述(copyCutMixed 单源门,跨源 no-op+toast 提示)。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/FavPickerDialog.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/components/ContextMenu.tsx ui/src/components/FavPickerDialog.tsx ui/src/components/FavPickerDialog.test.tsx ui/src/App.tsx ui/src/i18n/zh.json
git commit -m "feat(ui): 混合视图动作对等(跨源分组下载/删除·单源剪贴门)+目录添加到常用"
```

### Task 12: 拖放三路(内部拖入常用/外部拖入/粘贴进常用)

**Files:**
- Modify: `ui/src/App.tsx`(dragRef 多源化 1511-1541、onDragPointerUp 1461-1493、外部拖入 effect 1331-1353、requestPaste 抽取 1229-1269)
- Create: `ui/src/components/FavMemberPicker.tsx`
- Modify: `ui/src/components/FavView.tsx`(行 pointerdown 上抛源组)
- Test: `ui/src/components/FavMemberPicker.test.tsx`

**Interfaces:**
- Consumes: dragPlan/pastePlan/runTransfer 既有管线;T6 SourceSel。
- Produces: 拖放落点键两形:`fav\u0000<id>`(常用整体)与既有 `device\u0000path`(分组节/收件箱行);`pasteInto(device: string, path: string): void`(从 requestPaste 抽出,T11 粘贴菜单共用);FavMemberPicker props `{ open; fav: Favorite; devices: DeviceInfo[]; colors: Map<string,string>; onPick(member: FavMember): void; onClose(): void }`。

- [x] **Step 1: 写失败测试**

FavMemberPicker SSR:成员行=色点+设备名+路径;离线成员灰显仍可选(路由到远端上传由引擎拒,UI 不预判——简化:离线成员禁选,断言 disabled)。fan-out 断言(纯):无新纯函数,dragPlan 多源分组由 T6 groupRowsByDevice 已钉。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/FavMemberPicker.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

①dragRef 会话改 `sources: SourceSel[]`+`srcKey`(dir 位构造单组——零回归);FavView 行 pointerdown 上抛多源组(选中集跨源→按组)。②onDragPointerUp:key 解析——`fav\u0000` 前缀→该常用 members.length===1→目标=该成员(等效 device 键);>1→setMemberDropFor({favId, sources}) 开 FavMemberPicker,onPick→逐组 dragPlan(组.device→member.device/member.path)→runTransfer;device 键→逐组 dragPlan→runTransfer(跨源分组,伞形)。③外部拖入 effect 增 fav 位分支:drop→modalBusy 门(Review Focus #4)→`document.elementFromPoint(ev.payload.position.x / window.devicePixelRatio, y / devicePixelRatio)`(物理→CSS 像素)→closest [data-drop-loc]→同②;无命中忽略。④requestPaste 抽 pasteInto(device,path)(原 dir 位调用面零改);fav 位 Ctrl+V/菜单粘贴→members===1→pasteInto(该成员);>1→FavMemberPicker→onPick→pasteInto;canPaste(fav 位)=members.length>0。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/FavMemberPicker.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/App.tsx ui/src/components/FavMemberPicker.tsx ui/src/components/FavMemberPicker.test.tsx ui/src/components/FavView.tsx
git commit -m "feat(ui): 常用目录拖放三路(内部/外部 Explorer 拖入/粘贴)+多源分组 fan-out"
```

### Task 13: 管理屏(重命名/成员移除/删除常用)

**Files:**
- Create: `ui/src/components/FavManageDialog.tsx`、`ui/src/components/TreeNodeMenu.tsx`
- Modify: `ui/src/components/Sidebar.tsx`(右键菜单接线)、`ui/src/App.tsx`(onManageFav 装配)
- Modify: `ui/src/i18n/zh.json`
- Test: `ui/src/components/FavManageDialog.test.tsx`

**Interfaces:**
- Consumes: T4 renameFav/removeMember/deleteFav、T8 FavNameDialog。
- Produces: FavManageDialog props `{ open; fav: Favorite; devices: DeviceInfo[]; colors: Map<string,string>; busy: boolean; onRename(name: string): void; onRemoveMember(member: FavMember): void; onDelete(): void; onClose(): void }`;TreeNodeMenu props `{ x: number; y: number; items: { key: string; label: string; danger?: boolean; onPick(): void }[]; onClose(): void }`。

- [x] **Step 1: 写失败测试**

FavManageDialog SSR:成员行(色点+设备名+路径+移除钮);设备不在快照→「设备已移除」标注;删除钮=danger 文案「删除常用目录」;确认弹层文案含「只解除链接,不删除文件」。

- [x] **Step 2: 跑测试确认失败**

Run: `cd ui && npx vitest run src/components/FavManageDialog.test.tsx`
Expected: FAIL。

- [x] **Step 3: 实现**

Sidebar onManageFav 改为开 TreeNodeMenu(打开/重命名/管理成员/删除常用);重命名复用 FavNameDialog(initial=旧名);删除走 ConfirmDialog(spec 文案);FavManageDialog 内动作直接调 useFavorites op,成功后 store 自更新、弹层数据随 props 重投影。

- [x] **Step 4: 跑测试确认通过**

Run: `cd ui && npx vitest run src/components/FavManageDialog.test.tsx && npx tsc`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ui/src/components/FavManageDialog.tsx ui/src/components/TreeNodeMenu.tsx ui/src/components/FavManageDialog.test.tsx ui/src/components/Sidebar.tsx ui/src/App.tsx ui/src/i18n/zh.json
git commit -m "feat(ui): 常用目录管理屏(重命名/成员移除/删除=只解除链接)"
```

### Task 14: MTDSSKit FavoritesStore + 色板(Swift 镜像)

**Files:**
- Create: `ios/package/Sources/MTDSSKit/Data/FavoritesStore.swift`
- Modify: `ios/package/Tests/MTDSSKitTests/FakeDaemon.swift`(kv.get/kv.put 应答,若无)
- Test: `ios/package/Tests/MTDSSKitTests/FavoritesStoreTests.swift`

**Interfaces:**
- Consumes: `MtdssRepository.kvGet/kvPut`(已存)、DirectoryDevice。
- Produces(T15-T18 消费):`FavMember/Favorite/FavoritesDoc`(Codable,`CodingKeys` snake_case;`Favorite` 兼 `Hashable` 供 FavoritesRoute 携带)、`FavoritesPalette.hexes/colorMap(devices:)`、`@MainActor final class FavoritesStore: ObservableObject`——`@Published favorites: [Favorite] / readOnly: Bool / busy: Bool`;`refresh(repo:) / create(repo:name:) / rename(repo:id:to:) / delete(repo:id:) / addMembers(repo:id:members:) -> Int / removeMember(repo:id:member:)`(全部 put 成功才落,语义与 T4 逐条同)。

- [x] **Step 1: 写失败测试**

Mirror T4 六场景(含 Review Focus #1/#5):not_found→空;v2→readOnly 且 create 返回 false 且 **kv.put 零调用**;编辑前置取最新(注入他端新文档);put 失败保旧值;色板排序位次/回绕。

- [x] **Step 2: 跑测试确认失败**

Run(Mac mini): `cd ios/package && swift test --filter FavoritesStoreTests`
Expected: FAIL。

- [x] **Step 3: 实现**

JSONDecoder/Encoder snake_case;kvGet catch not_found → 空(FakeDaemon/真实 repo 错误面照 MtdssRepository.kvGet 注释);互斥 busy 门。

- [x] **Step 4: 跑测试确认通过**

Run(Mac mini): `cd ios/package && swift test --filter FavoritesStoreTests`
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ios/package/Sources/MTDSSKit/Data/FavoritesStore.swift ios/package/Tests/MTDSSKitTests/FakeDaemon.swift ios/package/Tests/MTDSSKitTests/FavoritesStoreTests.swift
git commit -m "feat(ios-kit): FavoritesStore+色板(整单 LWW·v 守卫·put 成功才落)"
```

### Task 15: AppModel 接线 + MixedViewModel(Kit)

**Files:**
- Create: `ios/package/Sources/MTDSSKit/Ui/MixedViewModel.swift`
- Modify: `ios/app/MTDSS/MTDSSApp.swift`(AppModel:T10 子模型区 + bootEngine 事件订阅)
- Test: `ios/package/Tests/MTDSSKitTests/MixedViewModelTests.swift`

**Interfaces:**
- Consumes: T14 store、T1 dataChanged、BrowseSort、MtdssRepository list/delete/transfer 面。
- Produces:

```swift
@MainActor public final class MixedViewModel: ObservableObject {
  public init(repo: MtdssRepository, favorite: Favorite, selfDeviceId: String?)
  public enum MemberStatus { case pending, ok, offline, failed }
  public struct MemberState: Identifiable { public let member: FavMember; public private(set) var status: MemberStatus; public private(set) var entries: [DirEntry] }
  public struct MixedRow: Identifiable { public let entry: DirEntry; public let device: String; public let key: String } // key = device + "\0" + path
  @Published public private(set) var states: [MemberState]
  @Published public private(set) var loading: Bool
  @Published public var sort: BrowseSort
  @Published public private(set) var isSelecting: Bool
  @Published public private(set) var selection: Set<String> // 复合键
  public func reload() async          // 渐进:offline 即定,pending 逐个落(seq 守卫)
  public func visibleRows() -> [MixedRow]  // mixed 排序投影;分组由 view 按 states 渲染
  public func enterSelection() / exitSelection() / toggle(_ key: String) / selectAll()
  public func downloadSelected() async -> Bool              // 逐组 download(收件箱哨兵,R12 ① 口径)
  public func sendSelected(to: DirectoryDevice) async -> Bool  // 逐组 SendService.send 同形
  public func deleteSelected(allowRemoteDelete: Bool) async -> Bool  // 逐组 deletePaths 同形
}
```

AppModel:增 `let favorites = FavoritesStore()`;refreshStatus 内 loggedIn→`await favorites.refresh(repo:)`;bootEngine 增一条 hub.subscribe() 流:`.dataChanged(key: "favorites")`→refresh。

- [x] **Step 1: 写失败测试**

MixedViewModelTests(FakeDaemon):渐进 offline 即定且 **list 不发**(Review Focus #3);应答并入;reject→failed;同路径跨源两行两 key(Review Focus #2);排序/分组投影;选择/动作分组调用形参断言(逐组 list/download/delete 请求序列)。

- [x] **Step 2: 跑测试确认失败**

Run(Mac mini): `cd ios/package && swift test --filter MixedViewModelTests`
Expected: FAIL。

- [x] **Step 3: 实现**

reload 语义镜像 favfetch.ts(offline 快照先定;逐成员 Task 并行 list,RpcGuard.withTimeout(BrowseViewModel.callTimeout 同值));排序比较器复用 BrowseViewModel 的排序实现(抽取或镜像,注明同源)。

- [x] **Step 4: 跑测试确认通过**

Run(Mac mini): `cd ios/package && swift test --filter MixedViewModelTests && swift test`(整包防回归)
Expected: PASS。

- [x] **Step 5: Commit**

```bash
git add ios/package/Sources/MTDSSKit/Ui/MixedViewModel.swift ios/package/Tests/MTDSSKitTests/MixedViewModelTests.swift ios/app/MTDSS/MTDSSApp.swift
git commit -m "feat(ios): MixedViewModel(渐进扇出/复合键选择/逐组动作)+AppModel favorites 接线"
```

### Task 16: iOS 混合页 UI(FavoritesMixedView)

**Files:**
- Create: `ios/app/MTDSS/Browse/FavoritesMixedView.swift`
- Modify: `ios/app/MTDSS/Browse/FavoritesView.swift`(FavoritesRoute 增 `.fav(Favorite)` 与 destination 分支——本 task 先由 T17 根列表触发,此处仅路由占位可编译)
- Test: 手动/冒烟钩子(`MTDSS_DEBUG_FAVMIXED=1` 模拟器自动推入截图面,沿 MTDSS_DEBUG_* 先例);编译验证 `ios/build.sh`(Mac mini)

**Interfaces:**
- Consumes: T15 MixedViewModel、DevicePickerSheet、BrowseList/BrowseEntryRow/BrowseNavBar/SelectionNavBar/SelectionActionBar 既有组件。
- Produces: `FavoritesMixedView(fav: Favorite)` 推入页:顶栏=常用名(返回=弹回首页);「···」=显示方式 Picker(混合/分组,`@AppStorage("mtdss.fav.org")`)/排序 Picker/选择;行=BrowseEntryRow 复用+行首来源色点;分组=节头行(色点+设备名+路径末段;离线/无法访问灰标)+节内行;子目录点按→推远端浏览(RemoteBrowseTarget)/本机成员子目录→子目录直入单目的页(FavoritesRoute.sub 同机制,rootPath=该子路径);动作底栏=[下载, 发送, 删除?](删除门=选择全为本机行恒可删(spec §6「本机成员行=本地删除」);选择含远端行须 allowRemoteDelete 开);下载直达收件箱、发送进 DevicePickerSheet、删除入确认。

- [x] **Step 1: 实现视图**

结构镜像 BrowseView/BrowseDestination 装配;色点=Circle fill(hex→Color 工具小函数,Kit FavoritesPalette.hexes 消费);DEBUG 钩子进页自动等待条目后截图(沿 FavoritesView debugExpandIfHooked 手法)。

- [x] **Step 2: 编译与既有测试零回归**

Run(Mac mini): `cd ios/package && swift test && cd ../app && ../build.sh build`(脚本参数以 build.sh 头注为准)
Expected: 构建成功、swift test 全绿。

- [x] **Step 3: Commit**

```bash
git add ios/app/MTDSS/Browse/FavoritesMixedView.swift ios/app/MTDSS/Browse/FavoritesView.swift
git commit -m "feat(ios): 混合页 UI(混合|分组·来源色点·钻取·下载/发送/删除门)"
```

### Task 17: iOS 常用标签根列表扩展 + 管理屏

**Files:**
- Modify: `ios/app/MTDSS/Browse/FavoritesView.swift`(根列表:常用行/左滑/··· 新建/空态 hint;推 .fav)
- Create: `ios/app/MTDSS/Browse/FavoriteManageSheet.swift`
- Test: 编译 + 冒烟钩子(`MTDSS_DEBUG_FAVLIST=1` 展开);逻辑面(左滑集/路由)以代码评审 + 冒烟手册项覆盖(视图薄)

**Interfaces:**
- Consumes: AppModel.favorites(T15)、FavoritesPalette、FavNameDialog 等价(SwiftUI alert+TextField)。
- Produces: 根列表=收件箱根行(现状)+常用条目行(名+成员色点串;点=推 .fav;左滑=[管理, 删除常用]);··· 菜单增「新建常用目录」;清单空→hint 行;FavoriteManageSheet(fav)={重命名(TextField+保存)、成员列表(色点+设备名+路径+移除;设备不在快照→「设备已移除」)、删除常用(确认 alert:只解除链接文案)}。

- [x] **Step 1: 实现**

左滑用 NativeSwipeActions 同款;下拉刷新既有 refreshable 增 `await model.favorites.refresh(repo:)`。

- [x] **Step 2: 编译与既有测试零回归**

Run(Mac mini): `cd ios/package && swift test && cd ../app && ../build.sh build`
Expected: 构建成功、测试全绿。

- [x] **Step 3: Commit**

```bash
git add ios/app/MTDSS/Browse/FavoritesView.swift ios/app/MTDSS/Browse/FavoriteManageSheet.swift
git commit -m "feat(ios): 常用标签根列表(收件箱置顶+常用行+管理/删除)+管理屏"
```

### Task 18: iOS 添加成员入口(远端浏览目录 → 常用)

**Files:**
- Modify: `ios/package/Sources/MTDSSKit/Ui/BrowseViewModel.swift`(164-214 动作枚举区)
- Modify: `ios/app/MTDSS/Browse/BrowseView.swift`(远端浏览页接线:swipe/选择底栏)
- Create: `ios/app/MTDSS/Browse/FavoritePickerSheet.swift`
- Test: `ios/package/Tests/MTDSSKitTests/BrowseViewModelTests.swift`(追加出集断言)

**Interfaces:**
- Consumes: BrowseSwipeAction/BrowseSelectionAction(增 case)、AppModel.favorites。
- Produces: `BrowseSwipeAction.favorite` / `BrowseSelectionAction.favorite`;新出集函数(既有 `actions(...)` 签名与出集**不变**,零回归):
```swift
public static func dirActions(entry: DirEntry, isRemote: Bool, allowRemoteDelete: Bool) -> [BrowseSwipeAction]   // = actions(...) + (entry.isDir ? [.favorite] : [])
public static func dirSelectionActions(isRemote: Bool, allDirs: Bool, allowRemoteDelete: Bool) -> [BrowseSelectionAction]  // allDirs 时追加 .favorite
```
BrowseView 增回调 `onAddFavorite(_ entries: [DirEntry]): Void`(swipe 单条/底栏多选);FavoritePickerSheet={清单(名+成员数)+点选→addMembers→toast;空态「新建常用目录」}。

- [x] **Step 1: 写失败测试**

BrowseViewModelTests 追加:dirActions 目录行含 .favorite、文件行不含;dirSelectionActions allDirs=true 含、false 不含;既有 actions() 出集逐字不变(零回归锚)。

- [x] **Step 2: 跑测试确认失败**

Run(Mac mini): `cd ios/package && swift test --filter BrowseViewModelTests`
Expected: FAIL。

- [x] **Step 3: 实现 + 跑测试确认通过**

Run(Mac mini): `cd ios/package && swift test && cd ../app && ../build.sh build`
Expected: PASS + 构建成功。

- [x] **Step 4: Commit**

```bash
git add ios/package/Sources/MTDSSKit/Ui/BrowseViewModel.swift ios/package/Tests/MTDSSKitTests/BrowseViewModelTests.swift ios/app/MTDSS/Browse/BrowseView.swift ios/app/MTDSS/Browse/FavoritePickerSheet.swift
git commit -m "feat(ios): 远端浏览目录添加到常用(滑删+选择底栏+选择弹层)"
```

### Task 19: 收尾——冒烟手册、spec 回填、全量回归

**Files:**
- Create: `docs/smoke-v3p3.md`(沿 docs/smoke-v3p1.md 体例)
- Modify: `docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md`(状态行→已实施)
- Modify: 本计划文件(勾选收口注记)

**Interfaces:**
- Consumes: 全部前序任务。

- [x] **Step 1: 冒烟手册编制**

Windows 实机项:新建→PC 添加成员→另一端打开混合/分组→同路径跨源着色→离线成员桶→外部 Explorer 拖入弹层→粘贴→管理屏重命名/移除/删除;iOS 真机项:常用行/混合页动作/添加成员/管理;双端互推(data_changed 秒级重拉)。真机/实机场 gate 标注挂用户侧。

- [x] **Step 2: 全量回归**

Run: `cargo test --workspace` + `cd ui && npm test && npm run build` + `cd android && ./gradlew.bat :app:testDebugUnitTest` + Mac mini `cd ios/package && swift test`
Expected: 全绿。

- [x] **Step 3: spec 状态行更新 + Commit**

```bash
git add docs/smoke-v3p3.md docs/superpowers/specs/2026-11-05-mtdss-v3p3-favorites-design.md docs/superpowers/plans/2026-11-05-mtdss-v3p3-favorites.md
git commit -m "docs(v3p3): 冒烟手册+spec 实施回填(V3-3 收口编制)"
```

---

> **收官注记(2026-11-06)**:T1-T19 全量落地收官(分支 v3p3;T19=收尾批 7 小修+冒烟手册 `docs/smoke-v3p3.md`+spec 状态行/实施回填);真机/实机场 gate 挂用户侧。
