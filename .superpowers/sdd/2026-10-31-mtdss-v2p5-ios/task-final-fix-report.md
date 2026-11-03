# R8 终审修复波报告(final-review fix wave:2 Important + 2 doc minors)

执行日期:2026-11-03。执行面:Windows(check + daemon 套件)+ WSL(photo 单测 + photo_serve_e2e + daemon 全量 + 排障复跑)+ Mac mini(swift test + app 构建)。

**状态:complete** — 四项全落地,8 commits(逐项提交,基 5a5f4c0 → `11f27a5`);三端验证绿(唯一败面为已记票的 `restart_marks_interrupted` 进度预算抖动家族,隔离复跑绿,非本波回归)。

## 1. Commits

| # | commit | 项 | 内容 |
|---|---|---|---|
| 1 | `401a007` | **I-1** | fix(photo): 0x06 批内唯一名收口 + 引擎重复 rel 兜底契约钉测 |
| 2 | `16770c4` | **I-2** | fix(photo): serve 目录迁 `{home}/photo-serve` 出暂存区命名空间 + photo-serve 保留首段 |
| 3 | `ee9e8f6` | **M-c** | docs(kit): SendService 陈旧注释更新至终态(Ruling J 已落地;v1 iOS 拒发 unix 桌面为裁定立场) |
| 4 | `e7e9816` | **M-d** | docs(spec): §3 收件箱「不可删除」句演进注记 + §10 根删除拒收加固票 |
| 5 | `7c45314` | I-1 复跑修正 | test(kit): 钉测期望改「DUP (1).JPG」(uniqueName 的 `(n)` 落扩展名前,与 InboxPublisher/ShareQueue 精确同语言;Mac 首跑实证 1 失败即期望笔误) |
| 6 | `aa4fd8d` | I-2 尾随 | docs(daemon): start_upload_bridge 校验注释随保留首段同步 |
| 7 | `8922889` | I-1 复审补 | test(daemon): 同名照片钉测补去重对照臂(桥答唯一名 → done 双照片逐字节) |
| 8 | `11f27a5` | I-2 自审拾遗 | docs(spec): §桥协议 0x07 路径随重定位更新 |

## 2. 逐项修复面

### I-1 — 重名照片硬败整册下载(App 列名去重 + 引擎安全网)

**选型声明:引擎侧保留硬 Err(两案中更小者),加契约钉测存档;未改 skip-ledger。**

- **App 侧**(答前收口,进桥应答之前):
  - `PhotoAssetNaming.uniqueName(_:used:)` 新增于 Kit(PhotoLibraryService.swift)——批内撞名 `name (n).ext` 顺延,与 InboxPublisher.nextAvailable / ShareQueue.uniqueInboxPath **逐字符同格式**;单一真源(PHPhotoLibraryService 导出的同名私有实现改委托,行为逐字节不变)。
  - `PhotoServe.answerList` 相册臂(PhotoServe.swift):回退命名 `IMG_<stamp>_<n>` 的序号改**批内真实顺位**(原恒 1,同刻无名照片回退名互撞)+ 逐条 `uniqueName` 收口——0x06 面逐条唯一,engine rel 面不再因 App 命名撞车硬败。dedupe 在帽截断之前,photo_id 跟资产走。
- **引擎侧**:`push_photo_item` 重复 rel 硬 Err 保持(现仅对非照片形态的真重复可达,如重名相册/伪桥直供)。
- **钉测**:
  - Kit(PhotoServeTests,+2):`testListAlbumDedupesDuplicatePhotoNames`(「DUP.JPG」「DUP (1).JPG」「OTHER.PNG」,序稳定 = 资产序,photo_id 不漂)、`testFallbackNamingUsesBatchIndex`(同刻无名照片回退名 `_1`/`_2` 天然互异)。
  - WSL e2e(photo_serve_e2e,+1 用例两面):`photo_album_duplicate_names_fail_cleanly`——①伪桥直供同名照片(绕过 App 去重)→ 相册下载编排期整单 failed(orchestrate accepted=false,文案点名「重复 rel」),**零 0x07 导出请求、接收端零落盘**(无部分传输泄漏);②对照臂:桥答批内唯一名「DUP.JPG」+「DUP (1).JPG」→ done,两照片各自 rel 逐字节落位。伪库加 `extra_albums` 特设路由注入面(不搅标准档 children 断言)。

### I-2 — photo-serve 撞收件暂存命名空间(重定位 + 协议保留段)

- **双锚重定位**:serve 目录 `{home}/.mtdss/photo-serve` → **`{home}/photo-serve`**(home 为 App 私有目录、Files App 不可见;**在 `.mtdss` 暂存区命名空间之外**——对端 sender 自选顶层 rel "photo-serve" 的收件中间态落 `<staging>/photo-serve`,原布局下挂载清扫/逐请求目录建删会在中断窗摧毁已暂存终件):
  - 引擎锚:`crates/daemon/src/photo.rs` `serve_dir()`;前缀判定(`path_under_serve_dir`)与清理面(`cleanup_photo_temps`)同锚自动随迁,单测矩阵更新(含「暂存区同名面不是 serve 目录内」新断言)。
  - App 锚:`ios/package/Sources/MTDSSKit/Engine/EngineLayout.swift` `photoServeDirectory` = `{home}/photo-serve`;sun_path 预算更短不受影响。布局测试改钉:在暂存区命名空间之外 + 不落发布根(`testPhotoServeDirectoryIsOutsideStagingNamespace`)。
  - WSL e2e serve 路径断言随迁(全链清扫/截断册清扫/evil 前缀外存活探测改 home 级兄弟位),模块文档同步。
- **协议纵深防御**(mirror `.mtdss` 保留段):`crates/core/src/protocol/manifest.rs` 新增 `reserved_first_segment(rel) -> Option<&'static str>`(`.mtdss`→intake、`photo-serve`→photo-serve;**大小写不敏感、仅首段、前缀形不误伤**),`validate_rel_path` 与 `verify`(手签清单双拒,错误信息点名保留语义)双双接入;`build_manifest` 发送侧同拒。
- **钉测**:core `photo_serve_first_segment_rejected`(build 拒/verify 拒含 "photo-serve"/大小写变体/裸名/`photo-server` 与深层 `sub/photo-serve/x` 不误伤);布局钉见上;WSL e2e 路径断言随迁即运行时 pin。

### M-c — SendService 陈旧注释

`SendService.swift` send 文档与 `PeerInboxTarget.unavailable` 文档改终态:T5b 携带的 NEEDS_CONTEXT 已由 **Ruling J** 收口(引擎侧哨兵统一:unix 收端 `dir="inbox"` 按非法回退默认收件目录,零协议变更,不再有「私有暂存+发布查询必超时」旧推理);**v1 iOS 对 linux/macOS 对端维持拒发为裁定立场**(对端落点非收件箱语义,宁拒勿误投),v1 后重评。零代码行为变化。

### M-d — spec §3「不可删除」句 + 加固票

- §3 收件箱行:注记 2026-11-03 R8——「不可删除」随 R5b 远端删除演进:对端可删授权根内文件(既有 ctrl delete 契约,UI 恒确认);收件箱根目录自身被删不自卫,自愈 = 下次收件照常重建落位(intake 落位 `create_dir_all` 实证)。
- §10 票池行追加:**收件箱根目录自身删除拒收**(hardening:引擎 delete 面对授权根自身路径逐条拒;现 `check_operand` 仅拒卷根/`.mtdss`)。纯文档。

## 3. 验证记录

| 面 | 结果 |
|---|---|
| Windows `cargo check --workspace --tests` | 绿,零警告 |
| Windows `cargo test -p mtdss-daemon` | 首跑(与 WSL/Mac 三线并发):lib **101/101** 绿 + ctrl_e2e **17/17** 绿;复跑(并发中)ctrl_e2e 2 例 punch 时序抖红 → **静默复跑 ctrl_e2e 17/17 复绿**;daemon_e2e 23/25(2 例 kill-resume 时序家族)→ **隔离复跑双绿**(35.5s/6.6s);其余二进制全绿 |
| Windows core manifest 单测 | **16/16** 绿(含新 `photo_serve_first_segment_rejected`) |
| WSL photo 单测(`--lib photo`) | **8/8** 绿(含重定位后 `serve_dir_prefix_matrix`) |
| WSL photo_serve_e2e | **6/6** 绿(67s;含同名照片硬败+去重对照双臂) |
| WSL daemon 全量(`-p mtdss-daemon --no-fail-fast`) | 30 二进制中 **29 绿**(lib 100 / ctrl_e2e 17 / photo_serve_e2e 6 / jobs 21 / relay 13+7 / inbox 系 / bridge 系…);唯一败 `daemon_e2e::restart_marks_interrupted`(24/25) |
| `restart_marks_interrupted` 排障 | **已记票抖动家族**(progress.md 后续轮票:「daemon_e2e restart_marks_interrupted 预算放宽(flake 家族)」;T5b 报告同款「进度预算型抖动,隔离恒绿」)——二连败后单测隔离复跑 **绿(27.15s)**;本波 Rust diff 零触及传输/中断路径(manifest 校验增量 + photo.rs 锚),非回归 |
| Mac `swift test` | 首跑 183 中 1 败 = I-1 钉测期望笔误(`7c45314` 修正);复跑 **183/183 零失败**(PhotoServe 17、EngineLayout 含新布局钉) |
| Mac `build.sh app`(app target) | 绿(产物 MTDSS.app;PHPhotoLibraryService 委托改编译实证) |

环境注记:WSL 经 `/root/mtdss-ci` 同步树(rsync,排除 .git/target);Mac 经 push macmini → ~/mtdss-ios reset --hard;MTDSS_UPDATE_URL 全程 unset。

## 4. Windows 静默复跑记录(补记,三线并发撤除后)

- `cargo test -p mtdss-daemon --no-fail-fast`:lib **101/101** 绿;**ctrl_e2e 17/17 绿**(并发轮两败复绿,确证负载抖动非回归);其余全数绿(1/7/21 jobs/6/3/6/13/14/12);**daemon_e2e 23/25**——败面 `restart_marks_interrupted` + `receiver_daemon_killed_sender_resumes`(同为 kill-resume 进度预算时序家族,R7 已记「该类在 base 与 my 树同形态;独占重跑全绿;非本波引入」)。
- 两败例 Windows 隔离复跑:**双绿**(`restart_marks_interrupted` 35.5s、`receiver_daemon_killed_sender_resumes` 6.6s)。
- **结论:三平台全绿;唯一抖动面为已记票的 kill-resume 时序家族(Win/WSL 同形态,隔离恒绿),与本波 diff 无涉(本波 Rust delta = manifest 保留段校验增量 + photo.rs serve 目录锚 + 注释/文档,零触及传输/中断路径)。**

## 5. Concerns(按重要性)

1. **重名相册不在 App 侧去重**(有意):相册匹配按原名(重名取先,v1 边界),改名第二同名相册只会造出不可寻址死条目;根层相册重名时「整库下载」仍引擎硬败(重复 rel),单相册下载/浏览不受影响。v1 边界与既有「重名取先」裁定一致,如需收口入票池。
2. **顶层名 "photo-serve" 的发送侧语义**:目录/文件顶层输入名 photo-serve 现 build_manifest 整单拒(「invalid rel path」,与 `.mtdss` 同款);flow.rs collect 侧「嵌套保留段静默跳过」**未**扩展到 photo-serve(嵌套 `sub/photo-serve/…` 合法照发,仅首段拒)——诚实失败而非静默丢件;若产品面希望静默跳过,并入 I-2 加固票一并处理。
3. **升级残件**:旧布局 `{home}/.mtdss/photo-serve` 下若残留升级前导出件(仅升级前传输中途崩溃才可能),新布局挂载清扫不再及之;经查 `publish_backlog` 按收件记录驱动(无记录件永不发布),残件为惰性垃圾不产生行为面。可选后续:App 清扫面顺带清旧路径一次。
4. **`restart_marks_interrupted` 抖动家族**(已有票):30s 进度预算在全量并行负载下可抖红,隔离恒绿;建议按既有票放宽预算或加预算内重试窗。
5. **Windows ctrl_e2e 并发抖动**:与 WSL/Mac 三线并发时 list/stat punch 两例超时败(同代码首跑绿);测试台负载敏感,非产品路径,静默复跑已复绿(见 §4)。
