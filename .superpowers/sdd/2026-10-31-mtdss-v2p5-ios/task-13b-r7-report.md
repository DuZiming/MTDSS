# T13b R7 功能波报告(Ruling P:照片图库对其他设备只读可读——0x06/0x07 照片桥 + 引擎虚拟根;App 内零照片 UI)

状态:**全交付,全量绿** / 基线 60b411d → HEAD(代码 4 commits;docs 收尾另计)
执行日期:2026-11-03。执行面:Windows(编排+自审+dev daemon)+ WSL(daemon 全量 e2e 红绿)+ Mac mini(swift test + build.sh sim + XCUITest + 截图 + sim E2E)。

## 1. Commits(顺序)

| # | commit | 内容 | 面 |
|---|---|---|---|
| 1 | `4cd6964` | feat(engine): fd 桥 0x06 photo_list / 0x07 photo_export + 照片图库虚拟根(photo.rs 状态机/ctrl 路由/jobs 混合源/接收面回退/ios-bridge 注入/photo_serve_e2e) | 引擎 |
| 2 | `77659bd` | feat(kit): PhotoResponder 应答器(权限门/列表/导出/清扫)+ Contract 事件 + FdPassKind 0x06/0x07 + EngineLayout.photoServeDirectory + 钉测 15 例 | Kit |
| 3 | `2f70a0a` | feat(ios): R5c 应用内照片 UI 全撤(第二固定行/图库页/网格/DEBUG 钩子)+ PhotoServe 装配 + 设置页权限入口(请求授权/撤权指引)+ GrantPhotos/BrowseList UITest 适配 | App |
| 4 | `9f8c82d` | fix(kit): 照片列表单帧条目帽 200 + truncated 上达(64KiB 帧防超限断连,自审) | Kit |

## 2. 交付对账(简报逐条)

- **桥协议**:fdpass 通道新两帧(App→daemon 应答,daemon 广播 `fs.photo_list {path,request_id}` / `fs.photo_export {photo_id,request_id}` 事件触发;与 0x01/0x04/0x05 同通道纪律,fdpass.rs 线格式表更新,kind 钉值测试补 0x06/0x07)。**0x06** 应答 `{entries,ok,request_id,truncated}`(条目 `{children?,is_dir,mtime_ms,name,photo_id?,size}`,字母序);**0x07** 应答 `{ok,path,request_id}`——App 导出**原始字节**(PHAssetResource,R5c `exportAssets` 复用)到 `{home}/.mtdss/photo-serve/<request_id>/` 回绝对路径,引擎按普通文件读发、**传输终态 unlink**(半途失败就地回收),App 挂载清扫兜底。
- **引擎虚拟根**:`DaemonOpts::photo_serve`(仅 ios-bridge 注入 true;桌面/Android 缺省 false = **照片臂整体不进,行为逐字节不变**)。`PhotoBridge`(photo.rs,新模块):登记先于广播/id 单调/超时即摘(迟到应答按未知 id ack ok:false)/列表缓存(完整虚拟路径→条目+photo_id;stat 命中即回,miss 重列父目录;`MTDSS_TEST_PHOTO_TIMEOUT_MS` 注入,list 10s/export 60s)。ctrl 面:列根与授权根并列合成「照片图库」(children=相册数,**桥不可用→根隐藏**,每次访问重探);`/照片图库` 前缀(词法规范化后,`/照片图库/../x` 规范化落 fs 面无逃逸)List/Stat 路由桥,**Delete/Move/Copy/CleanupOutbox 一律拒**(`INVALID_PATH 照片图库为只读来源…`,与开关无关);接收面 target_dir 指向照片根 → warn + 回退默认收件目录(坏 dir 不拒收同语义)。jobs 面:照片输入经 0x06 解析成 PhotoItem(文件输入 rel=文件名/目录输入 rel=目录名/内部相对,与 collect_items 同语义;整库输入相册层一层展开;rels 含目录项与 `local_tree_rels` 同口径;partial 重发模式顶层消失收 missing)→ **真路径+照片可混单**(`UploadSource::Mixed`),驱动逐件 0x07 导出临时件后走既有清单/块流/续传全链。stat = 缓存命中/miss 重列。
- **App 侧**:PhotoLibraryView/PhotoGridView/首页第二固定行/`MTDSS_DEBUG_PHOTO(_TAB)` 钩子全撤(`FavoritesRoute.photoLibrary` 随删;BrowseListUITests 首页标记改收件箱行);**保留** PhotoLibraryService 协议 + PHPhotoLibraryService 绑定 + 原始字节导出(PhotoAssetNaming 随迁;PhotoAsset 增 `resourceSize` = PHAssetResource KVC fileSize,0x06 文件项 size)。**PhotoResponder**(Kit):权限门先行(**非 authorized/limited 一律 ok:false** → 引擎按桥不可用收口 → 虚拟根隐藏;每次访问现查现答,无通知面——授权翻转=下一问即拒);根=相册目录项(children=照片数)/相册名=照片文件项(name/size/mtime/photo_id,photo_id=资产 localIdentifier = 0x07 凭据);导出逐请求子目录防在途覆写;挂载清扫。装配 = boot 与 InboxPublish 同位双订阅流(**@MainActor**:PHPhotoLibraryService 主线程裁定 R5c 坑2)。设置页「照片图库」段 = **权限入口**:未询问→行内「请求授权」(requestAccess);denied→前往设置指引;authorized/limited→状态+系统设置深链;变化流活体跟随;`MTDSS_DEBUG_PHOTO_PROMPT` 钩子迁设置页。
- **门控**:responder 应答权 = 照片权限 authorized/limited;else 拒答→引擎根隐藏,重探自然发生(权限翻转眼见)。

## 3. 验证矩阵(全绿)

| 面 | 结果 |
|---|---|
| Windows `cargo check --workspace --tests` | 绿,0 error/0 warning |
| Windows daemon lib | **99/99** |
| **WSL 全量**(daemon+ipc+cli+ios-bridge,`--no-fail-fast`,~/mtdss-ci 同步树) | **EXIT=0,30 个测试二进制全绿零失败**(daemon lib 97[含 photo 单测 3]、ctrl_e2e 17、daemon_e2e 25、jobs 21、relay 13、inbox×2、**photo_serve_e2e 3**、cli 28、bridge 3+4、upload_bridge 4…) |
| Mac `swift test` | **181/181 零失败**(VM 17 撤,PhotoServe 15 + EngineLayout 1 增) |
| Mac sim 构建 + 装机启动 | 绿(xcodebuild sim,app pid 49843;R7 树经 push macmini→reset --hard) |
| XCUITest | **GrantPhotos 1/1**(TAB=profile+AUTOPUSH=settings 新入口驱动)+ **BrowseList 7/7**(收件箱行新首页标记;远端滑序 M-4 环境 skip 如前) |
| **sim E2E(Windows dev daemon 44d97f96 → sim 380d76e3,产线信令真登录)** | 列根=`Documents + 照片图库`;列相册=最近项目(12 张,size=资源字节数/mtime=拍摄时间);**下载 IMG_0011/0012 两张,md5 与 addmedia 落盘原件逐字节一致**(908F446A…/9D6E8992…);`rm` 照片路径拒(`INVALID_PATH 照片图库为只读来源`);`put` INTO /照片图库 → **回退默认收件箱落位**,`/照片图库` 实体目录零产生;传后 `photo-serve` **0 文件**(引擎 unlink 实证,残目录挂载清扫收) |
| 截图/物料 | `artifacts/mtdss-r7-settings.png`(照片图库权限=已授权+撤权指引)、`artifacts/mtdss-r7-pc-listing.txt`(PC 侧列根/列相册/列相册内 + md5 对账) |

## 4. TDD 轨迹与排障记录(实证防复发)

1. **photo.rs 单测 phase③ 死锁(红)**:缓存 miss 用例误用已被上一步整表落缓存的条目 → stat 命中不发事件 → 测试 `rx.recv()` 永挂(全 lib 套件卡死 60s+,曾连续两轮误诊为环境问题)。修=未缓存条目(IMG_3)作 miss 面;单测 0.3s 绿。
2. **e2e 两处测试 bug(红)**:①浏览调用误走 A 自己的 IPC client(daemon `self` 别名短路绕开 ctrl 面,fs NotFound 暴露)——全部改 B 端 client(含三处多行形态,首刀 sed 只中单行);②混合下载断言误用目录输入 rel(文件输入 rel=文件名落目标根,与真路径文件同语义)。修后 3/3 绿。
3. **WSL 假失败三连(环境,非代码)**:photo 单测死锁被 TaskStop 强杀后,残留进程/信令态使后续 ipc_cli `update_state_stream` 90s 超时、daemon_e2e 3 例 punch 超时——**base(60b411d)与 my tree 在清场后独占重跑全绿**(base 三二进制 17+25+9 绿;mine 同位 17+25+9+3 绿;cli 单测 mine 0.14s 绿)。教训:强杀 cargo test 后必须清残留进程再判绿红。
4. **64KiB 帧帽(自审,commit 4)**:0x06 条目约 170-200B,大相册(>~370 张)不设帽即超限触发协议断连。Responder 按帽 200 截断保前缀(照片新→旧)+ truncated=true 透传(fs 列表同语义);下载展开同吃帽内前缀。
5. **Git Bash/PowerShell 传 CJK 路径坑(测试台,非产品)**:MSYS 路径转换把 `/照片图库` 改写成 `C:/Program Files/Git/照片图库` → `MSYS2_ARG_CONV_EXCL="*"` 禁转换后全链通。

## 5. Concerns(按重要性)

1. **桥缺席的列根探针等待**:App 死/未授权时,守卫对端每次列根都吃满 list 超时(生产 10s;测试注入 1.5s)才返回(根隐藏)。无负缓存(每次访问重探=权限翻转眼见)。对端体验:浏览未授权 iOS 设备根列表慢一拍(10s)后只见 Documents。若真机反馈差,候选=探针负缓存 TTL(1-5s)再票。
2. **单帧帽边界**:>200 张相册的浏览/整册下载只见面 200 张(truncated=true;下载同理只传前 200)。分页协议不做(v1);个人大库用户可按相册拆分。真实照片文件名+id 条目实测 ~170B,200 条 ≈ 35KB,帽内余量充足。
3. **PHAssetResource KVC "fileSize"**(assets 列表的 size 声明值)与**逐资产 resource 枚举**(大相册列表延迟,1000 张≈秒级)都是事实口径面;清单以导出件 fstat 为准,偏差无害。真机大库性能留真机轮。
4. **limited 授权**:子集由 PhotoKit fetch 天然收口(仅许可资产在列),「选择更多照片」系统入口未接(R5c Park 承袭)。
5. **真机未验证面**:真机照片库(几万张)列根/列表延迟、iCloud 未下载资产 0x07 导出走网络拉取(慢,无进度)、权限撤销瞬间在途导出的失败文案——留真机轮。
6. **测试台残留**:C:\temp-r7(脚本/md5 对账底稿,库外,可整删);~/mtdss-ci 已被 rsync 成 R7 树(base 留档 /root/r7-base);dev daemon(44d97f96)已按命令行匹配杀停,**产线 57700 未触碰**;sim 收件箱 IMG_0007 (n).JPG 为历史轮产物未动。

## 6. 产物清单

- 引擎:`crates/daemon/src/photo.rs`(新)、`ctrl.rs`(守卫路由+三臂)、`jobs.rs`(FdBridgeHandler 0x06/0x07+Mixed 源+解析/驱动/清扫)、`flow.rs`(接收回退)、`roots.rs`(normalize_lexical pub(crate))、`state.rs`/`lib.rs`/`main.rs`(photo_serve 注入链)、`crates/ipc/src/fdpass.rs`(kind)+ `crates/ios-bridge`(true)
- 测试:`crates/daemon/tests/photo_serve_e2e.rs`(新 3 例)+ photo.rs 单测 5 + fdpass 钉值 + 全 daemon opts 线程
- Kit:`Photo/PhotoServe.swift`(新)、`PhotoLibraryService.swift`(重定位)、Contract/FdPassChannel/EngineLayout 增量;`PhotoServeTests` 15 例;VM+VM 测试撤除
- App:`Photo/PhotoServeMount.swift`(新);FavoritesView/AppTab/MTDSSApp/SettingsView/PHPhotoLibraryService;GrantPhotos/BrowseList UITest 适配
- 文档:spec §4.8 重写(Ruling P)+ progress 状态行 + 本报告
- 截图/物料:`artifacts/mtdss-r7-settings.png`、`artifacts/mtdss-r7-pc-listing.txt`
