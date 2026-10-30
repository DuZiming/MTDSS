# MTDSS 1.1.0 打包轮运维手册(物料 / 部署 / 使用 / 命令 / 测试)

> 基线:main@`39c6fe2`(PR #21 merge)之上,本轮打包提交链(全部已推送,CI 终态整板绿):
> `88c4a9f`(gitignore 私钥)→ `3d1c753`(版本 1.1.0)→ `91ede8b`(tauri 锁)→ `78a978e`(golden REGEN)→ `f8df7db`(Kotlin 断言)→ `1896a0a`/`eff2291`/`a2eea6c`/后续 docs(本手册及回填)。
> 域名 `www.mtdss.top`(VPS 47.105.103.246)。
> 本轮产物全部本地构建 + 本地签名;CI 侧 release workflow dispatch 演练已跑(见 §1.4)。
> 配套文档:`scripts/deploy/README.md`(服务端资产)、`docs/smoke-m4.md`(更新链判定)、`docs/smoke-p4b.md`(UI 三档)。

## 0. 本轮构建要点(为什么这批产物是可信的)

| 要点 | 说明 |
| --- | --- |
| 版本基线 | 四处版本文件(根 `Cargo.toml` / `ui/package.json` / `ui/src-tauri/tauri.conf.json` / `ui/src-tauri/Cargo.toml`)全部 1.1.0;golden `hello.resp.json` 版本串已随 REGEN 再生 |
| 域名烤入 | Windows/Linux 客户端构建期注入 `MTDSS_SIGNALING_URL=https://www.mtdss.top`、`MTDSS_UPDATE_URL=https://www.mtdss.top/dl`(运行时 env 优先;详见 `crates/ipc/src/consts.rs`)。已用 strings 抽验:二进制内只有域名,无旧 IP、无 `.invalid` |
| 更新验签公钥 | Windows daemon 构建期注入 `MTDSS_UPDATE_PUBKEY`(= `keys/update-pub.hex`);缺失则更新通道 fail-closed |
| manifest 签名 | `mtdss-sign manifest → sign(update-priv.hex) → verify(update-pub.hex)` 全链本地完成;URL 基址 `https://www.mtdss.top/dl` |
| 服务端测试 | signaling + stun-server 测试套 59/59 绿(WSL);本地起服冒烟全过(端口/账号预置/中继配置面/auth 404-200 往返,脚本见 §5.1) |
| ⚠ 已知缺口 | **release.yml 不注入上述两个 URL**(CI 产物烤 `.invalid` 占位,fail-closed)——**首真 tag 前必须补**(跟进票,见 §6) |

## 1. 物料清单(东西在哪里)

全部物料根目录:`smoke/pkg-1.1.0/`(gitignored,不进库)。

### 1.1 交付产物

| 产物 | 路径 | 校验 |
| --- | --- | --- |
| Windows NSIS 安装器 | `smoke/pkg-1.1.0/win/MTDSS_1.1.0_x64-setup.exe`(6.0 MB) | sha256 `e167a50d2a4c4055a286abe71dfbc35b996e29ad09c19de3206cf30e92580433` |
| 更新清单(已签名) | `smoke/pkg-1.1.0/win/manifest.json` | `mtdss-sign verify` 通过;内含上述 sha256 |
| Linux 客户端 tar | `smoke/pkg-1.1.0/linux/mtdss-1.1.0-x86_64-linux.tar.gz`(8.3 MB) | 解包冒烟通过(§5.2) |
| 服务端三件套 | `smoke/pkg-1.1.0/server/{signaling-server,mtdss-admin,stun-server}`(Linux x86_64) | 测试 59/59 + 起服冒烟(§5.1) |
| Android APK(debug) | `android/app/build/outputs/apk/debug/app-debug.apk`(versionCode 2 / 1.1.0) | 装机前核对签名证书(§3.3) |
| **VPS 上传捆** | `smoke/pkg-1.1.0/vps-upload/`(服务端三件 + manifest.json + setup.exe,一次 scp 全带走) | — |

### 1.2 密钥

| 文件 | 保管 |
| --- | --- |
| `keys/update-pub.hex` | 入库(公钥,构建期注入源) |
| `keys/update-priv.hex` | **仅本地,已 gitignore(`/keys/*-priv.hex`),离线保管,严禁入库** |
| Android release keystore | 用户离线保管;本轮 debug APK 不涉及(§3.3 有 release 签名步骤) |

### 1.3 构建脚本(复现本轮构建)

| 脚本 | 用途 |
| --- | --- |
| `smoke/pkg-1.1.0/wsl-build.sh` | WSL:服务端构建+测试+客户端构建+打 tar(首跑) |
| `smoke/pkg-1.1.0/wsl-rebuild.sh` | WSL:客户端重烤(烤域名)再打 tar |
| `smoke/pkg-1.1.0/wsl-server-test.sh` | WSL:服务端起服冒烟(端口/账号/中继/auth) |
| `smoke/pkg-1.1.0/wsl-tar-test.sh` | WSL:tar 解包冒烟 |
| Windows 侧 | `unset MTDSS_UPDATE_URL MTDSS_SIGNALING_URL` 后 export 三变量(PUBKEY/SIGNALING/UPDATE)→ `cargo build --release -p mtdss-cli -p mtdss-daemon -p mtdss-sign` → `cd ui && npx tauri build` → `mtdss-sign manifest/sign/verify`(命令模板见 §4.5) |
| Android 侧 | PowerShell:`$env:ANDROID_HOME=...; .\gradlew.bat assembleDebug`(gradle 自带 cargo-ndk 引擎编译) |

### 1.4 GH / CI 侧(本轮已配齐)

| 项 | 值 |
| --- | --- |
| vars `MTDSS_DL_BASE` | `https://www.mtdss.top/dl`(已从旧 IP 修正) |
| vars `MTDSS_UPDATE_PUBKEY` | update-pub.hex 内容(已配) |
| secret `MTDSS_SIGNING_KEY` | update-priv.hex 内容(已配,写后不可读,可删重配) |
| deploy secrets(KEY/HOST/USER) | **未配**(无 VPS SSH 通道)→ publish 跳过 VPS 上传仅发 GH Release,设计如此 |
| release workflow 演练 | workflow_dispatch 已跑;**注意:CI 产物烤 `.invalid` URL 占位,不能直接当生产件用**(§6 缺口) |

## 2. VPS 部署(用户操作,约 10 分钟)

> 前提:VPS 可 SSH(阿里云控制台或既有通道);`www.mtdss.top` DNS/TLS 已就绪(已验证 /dl 可达)。
> 全部命令在 VPS 上以 root 执行,除标注 `sudo -u mtdss` 的两条。

### 2.1 上传(在 Windows 上执行)

```powershell
# 从仓库根目录;用户名/端口按实际改(root 或既有部署账号)
scp -r smoke\pkg-1.1.0\vps-upload root@47.105.103.246:/tmp/pkg-1.1.0
```

### 2.2 服务端更新(备份 → 就位 → 重启 → 验证)

```bash
# 0) 备份旧件(回退用)
mkdir -p /root/backup-$(date +%m%d)
cp -a /usr/local/bin/signaling-server /usr/local/bin/mtdss-admin /usr/local/bin/stun-server /root/backup-$(date +%m%d)/

# 1) 就位新二进制(1.1.0,含 v2 全量服务端逻辑)
install -m 755 /tmp/pkg-1.1.0/{signaling-server,mtdss-admin,stun-server} /usr/local/bin/

# 2) relay 三件套之一:unit 补 --relay-addr(中继通告地址,不补则通告 127.0.0.1 永不可用)
#    编辑 /etc/systemd/system/mtdss-signaling.service 的 ExecStart,改为:
#    ExecStart=/usr/local/bin/signaling-server --plain-http 127.0.0.1:8787 \
#        --stun-addr 47.105.103.246 --stun-port 3478,3479 \
#        --relay-addr 47.105.103.246 /var/lib/mtdss/signaling.db
#    (即:在现有行上补 --relay-addr 47.105.103.246 一段)
vim /etc/systemd/system/mtdss-signaling.service
systemctl daemon-reload && systemctl restart mtdss-signaling mtdss-stun

# 3) 验证
systemctl status mtdss-signaling mtdss-stun --no-pager | head -20
journalctl -u mtdss-signaling -n 5 --no-pager        # 应见 relay data plane on udp 47.105.103.246:8788
ss -ulnp | grep -E ':(8788|3478|3479)\b'             # 8788 须绑在公网 IP 上
```

### 2.3 relay 三件套之二/三:安全组 + 账号开通

```bash
# 之二:阿里云安全组放行 UDP 8788(控制台操作,脚本做不了;不放行则中继兜底不可用)
# 之三:账号开通中继(默认 disabled;DB 是 WAL,运行中直接执行)
sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db relay enable kk@test.com
# 按需调限速/配额(默认 1024 kbps / 10 GiB;0=不限):
sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db relay limit kk@test.com 2048
sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db relay quota  kk@test.com 20
sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db user list --with-usage
```

### 2.4 /dl 更新源(Windows 客户端自动更新的数据源)

```bash
mkdir -p /var/www/mtdss/dl/win/1.1.0
install -m 644 /tmp/pkg-1.1.0/manifest.json /var/www/mtdss/dl/manifest.json
install -m 644 /tmp/pkg-1.1.0/manifest.json /var/www/mtdss/dl/win/1.1.0/manifest.json   # 版本钉扎位(回滚/重装用)
install -m 644 /tmp/pkg-1.1.0/MTDSS_1.1.0_x64-setup.exe /var/www/mtdss/dl/win/1.1.0/
# (可选)清理旧 IP 形态死链:旧 /dl/manifest.json 已被上面覆盖;win/1.0.1/ 留作回滚源
```

### 2.5 部署后验证(在任意机器)

```bash
curl -s https://www.mtdss.top/dl/manifest.json | head -3      # 应为 1.1.0、URL 域名形态
curl -s -o /dev/null -w '%{http_code} %{size_download}\n' \
  https://www.mtdss.top/dl/win/1.1.0/MTDSS_1.1.0_x64-setup.exe   # 200 6267752
curl -s -X POST https://www.mtdss.top/auth/request-code \
  -H 'content-type: application/json' -d '{"email":"kk@test.com"}'   # 200 + dev 码(未预置邮箱 404)
```

### 2.6 回退

```bash
# 服务端:cp -a /root/backup-<日期>/... /usr/local/bin/ 后 systemctl restart mtdss-signaling mtdss-stun
# /dl:把 /var/www/mtdss/dl/win/1.0.1/manifest.json(若存)复制回 /dl/manifest.json;或重签旧版清单
# 客户端:mtdss update 1.0.1(版本钉扎回滚,需 /dl/win/1.0.1/manifest.json 在位)
```

## 3. 各端安装与使用

### 3.1 Windows(本机现为 1.0.1 加固版)

- **推荐路径:走更新链**(真实验收):托盘「检查更新…」→ 应发现 1.1.0 → 下载(经域名 /dl)→ 验签 → 安装 → 重启 → 版本翻新。细节判定见 `docs/smoke-m4.md` §B/§I。
- 备选:直接装 `MTDSS_1.1.0_x64-setup.exe`(NSIS 覆盖升级,数据保留)。
- **本机注意**:用户级环境变量 `MTDSS_UPDATE_URL` 已从旧 IP 改为 `https://www.mtdss.top/dl`(2026-10-30);已运行的旧 daemon 需重启(退出托盘重开/注销重登)才会读到新值。
- 登录:UI 打开即预填 `https://www.mtdss.top`(r2f 起默认域名);邮箱+验证码即可,服务器/CA/设备名在「高级选项」。
- 数据目录:`%USERPROFILE%\.mtdss`(`MTDSS_HOME` 可覆盖);daemon 日志缺省走 stderr(自启形态不落盘)——排障时退出托盘后手动前台跑 `mtdssd --log-file daemon.log` 复现。

### 3.2 Linux(WSL 或任意 x86_64 ≥ glibc 2.25 主机)

```bash
tar xzf mtdss-1.1.0-x86_64-linux.tar.gz     # 得 mtdss / mtdssd / README.txt
./mtdssd --no-tray --log-file mtdssd.log &  # 守护进程(常驻;收件目录默认 ~/.mtdss/inbox,或 --recv-dir)
./mtdss login --email kk@test.com            # 服务器默认已烤域名;终端回显 dev 验证码,输入即登录
./mtdss devices                              # 应见账号下全部在线设备
```

- tar 平台**无自动更新**,升级=重新下载替换。
- 家 Linux 主机同样解包即用;glibc 下限以 Ubuntu 22.04(2.35)为准,更老发行版自测。

### 3.3 Android(MIX 3 / arm64 / Android 10+)

```powershell
# 装机(debug 直装,覆盖升级,数据保留;证书不同则需先卸载)
adb install -r android\app\build\outputs\apk\debug\app-debug.apk
# 核对版本
adb shell dumpsys package com.mtdss.android | findstr /C:version
```

- 登录:邮箱+验证码;服务器地址在高级选项,预填 `https://www.mtdss.top`(公共 CA,无需自签 CA)。
- **本轮装机必须重装 APK**:旧装机早于 r2g「公共 CA panic 修复」,切域名登录大概率 panic。
- 应用内「检查更新」走 daemon manifest 同源(fail-closed 属预期,见 §6)。
- **首发前 release 签名**(私钥离线保管,四属性注入):
  ```powershell
  cd android
  $env:MTDSS_RELEASE_STORE_FILE="<jks 路径>"; $env:MTDSS_RELEASE_STORE_PASSWORD="<口令>"
  $env:MTDSS_RELEASE_KEY_ALIAS="mtdss"; $env:MTDSS_RELEASE_KEY_PASSWORD="<口令>"
  .\gradlew.bat assembleRelease   # → app/build/outputs/apk/release/app-release.apk
  ```

## 4. 常用命令速查

### 4.1 `mtdss`(客户端 CLI;对端在线经本机 daemon)

| 命令 | 作用 |
| --- | --- |
| `mtdss login --email <邮箱> [--server <URL>] [--ca <PEM>] [--name <设备名>]` | 登录(交互输验证码;缺省服务器=烤入域名) |
| `mtdss logout` / `mtdss status` / `mtdss devices` | 登出 / daemon 状态 / 账号设备清单 |
| `mtdss ls <设备>:<远端路径>` | 浏览远端目录 |
| `mtdss send <本地路径> <设备>[:<目录>]` | 发送(对端确认落 recv 目录) |
| `mtdss put <本地> <设备>:<scp形目标>` / `mtdss get <设备>:<远端> <本地目录>` | 作业化上传/下载(阻塞至终态) |
| `mtdss cp <源设备>:<路径> <目标设备>:<目录>` / `mtdss mv` / `mtdss rm` | 设备间复制 / 同设备移动 / 删除(缺省进回收站) |
| `mtdss recv <目录>` | 设常驻接收目录 |
| `mtdss updatecheck` / `mtdss update [版本]` | 检查更新 / 应用更新(缺省最新;带版本=钉扎回滚) |

### 4.2 `mtdssd`(守护进程)

`--recv-dir <目录>`(缺省 settings.json → `{home}/inbox)`、`--log-file <文件>`(按行 JSON)、`--no-tray`。home=`MTDSS_HOME` → `~/.mtdss`。

### 4.3 `mtdss-admin`(VPS 运维,生产记得 `sudo -u mtdss` + `--db /var/lib/mtdss/signaling.db`)

`user add/remove/list [--with-usage]` · `relay enable/disable <email>` · `relay limit <email> <kbps>`(0=不限,默认 1024) · `relay quota <email> <GB>`(0=不限,默认 10)。WAL 并存,运行中可直接执行;全局带宽 env `MTDSS_RELAY_GLOBAL_KBPS`(默认 2500)。

### 4.4 服务端/系统(VPS)

`systemctl status|restart mtdss-signaling mtdss-stun caddy` · `journalctl -u mtdss-signaling -f` · `ss -ulnp | grep 8788` · 端口口径:TCP 443(Caddy)、UDP 3478/3479(STUN)、UDP 8788(中继)、8787 仅回环**严禁对外**。

### 4.5 签名/打包复现(mtdss-sign)

```bash
mtdss-sign keygen --out <离线目录>                       # 已有则拒,防误覆盖
mtdss-sign manifest --version <v> --url-base https://www.mtdss.top/dl \
  --notes-file notes.txt --file MTDSS_<v>_x64-setup.exe --out unsigned.json
mtdss-sign sign --manifest unsigned.json --key keys/update-priv.hex --out manifest.json
mtdss-sign verify --manifest manifest.json --pubkey keys/update-pub.hex
```

### 4.6 adb(MIX 3=be81fc86;Git Bash 一律 `MSYS_NO_PATHCONV=1`)

`adb devices` · `adb install -r <apk>` · `adb logcat -s mtdssd MTDSS | grep -i <词>` · `adb shell dumpsys package com.mtdss.android`(版本) · MIUI 语义库见 `docs/smoke-p4a.md`。

## 5. 整体测试清单

### 5.1 已完成(本轮本地验证,2026-10-30)

- [x] 服务端测试套 59/59(WSL,signaling+stun)
- [x] 服务端起服冒烟:8787/8788/3478/3479 监听、user add/relay enable/limit/quota、auth 未预置 404 + 预置 200(dev 码回显)、日志无错
- [x] Linux tar 解包冒烟:mtdss/mtdssd 版本 1.1.0、独立 MTDSS_HOME 起动、`status` 应答
- [x] 二进制烤入抽验:仅域名 URL,无旧 IP / `.invalid`
- [x] manifest 签名验签(1.1.0,域名 URL,sha256 与 setup.exe 一致)
- [x] golden 契约随版本再生,本地绿(Rust + Kotlin 断言 5 处联动)
- [x] **Linux 客户端域名登录 E2E**:WSL 解包 tar → pty 自动应答验证码 → 登录成功,`online=true`、`server=https://www.mtdss.top`(烤入默认值直连现役 VPS),设备清单互见(含 MIX 3)
- [x] **MIX 3 已装 1.1.0**(versionCode 2,debug 证书与旧装一致,数据保留;r2g 公共 CA 修复就位)
- [x] CI release dispatch 演练全绿(三平台产物:windows/linux/macos-dist)
- [x] ci.yml 整板绿(五 job;golden/Kotlin 联动修复验证;`update_state_stream_three_branches` 90s 超时再现一次=留验 flake 家族,rerun 绿)

### 5.2 VPS 部署后(用户跑完 §2 喊我继续)

| # | 项 | 判定 |
| --- | --- | --- |
| T1 | §2.5 三条 curl | manifest=1.1.0 域名形态;setup 200;auth 200 |
| T2 | Windows 更新链 | 本机 1.0.1 → 检查更新 → 1.1.0 → 安装 → 版本翻新(smoke-m4 §B) |
| T3 | 三端域名登录 | Windows UI / WSL CLI / MIX 3 App 全部 `kk@test.com` 登录,`devices` 互见 |
| T4 | 直连传输矩阵 | Win↔WSL、Win↔手机 双向 send/put/get,md5 抽查一致 |
| T5 | 中继兜底 | 打洞失败场景自动走 relay(抽屉角标/日志证);前提 §2.3 全做 |
| T6 | 任务抽屉/续传 | 传输中断 → 任务列表「中断」态 → 重新发起续传(P2/P4a 语义) |
| T7 | smoke-p4b B/C 档 | 按 `docs/smoke-p4b.md` 三档清单(主题/排序/拖拽/地址栏/抽屉) |
| T8 | 回滚演练(可选) | `mtdss update 1.0.1` 钉扎回滚,数据保留 |

### 5.3 记录格式

逐项记 通过/失败+现象+日志位置(Windows:daemon `--log-file`;Linux:mtdssd.log;VPS:`journalctl -u mtdss-signaling`;手机:`adb logcat`)。失败先查 §7。

## 6. 已知边界与跟进票(不阻塞本轮)

1. **release.yml 不烤 URL**(CI 产物 `.invalid` 占位)——首真 tag 前补:build-windows/build-linux 注入 `MTDSS_SIGNALING_URL`/`MTDSS_UPDATE_URL`(从 vars 取值,同 PUBKEY 模式;注意 ipc 占位测试不被污染的既有裁定)。
2. **版本翻新清单**(下次 bump 照此扫):四处版本文件 + `Cargo.lock`×2 + golden `hello.resp.json`(REGEN)+ Kotlin 断言 5 处(ContractGoldenTest 1 + IpcClientTest 4)+ `android/app/build.gradle.kts` versionCode/versionName——本轮已全部落位,漏一处 CI 对应 job 即红。
2. CI flake 家族(update_state_stream 90s / reinitiate ConnectAccept / inbox 1MiB 账差)治本票仍挂。
3. tar 平台无自动更新(设计);macOS aarch64 仅 CI 能出(本地无交叉链)。
4. Android 应用内检查更新复用 Windows manifest(`/win/` 段),首发前单列 APK 更新通道(P3 遗留口径)。
5. debug APK 为测试形态;首发必须 release 签名(§3.3 步骤)+ 签名密钥离线保管。
6. 旧 IP 形态 TLS 已死(仅本机旧金丝雀受影响;env 已切域名)。

## 7. 故障速查

| 现象 | 先查 |
| --- | --- |
| 登录连不上 | `curl https://www.mtdss.top/auth/request-code`(§2.5);UI 高级选项里服务器拼写;证书是否公共 CA(r2g 已修 panic,旧装机必须换 APK) |
| 检查更新报「未配置」 | 该 build 未注入 PUBKEY(或 manifest 验签失败);确认装的是本轮 setup.exe |
| 检查更新拉不到 | daemon 运行时 env 覆盖:确认 `MTDSS_UPDATE_URL` 无旧 IP 残留(本机已改域名);`/dl/manifest.json` 是否 1.1.0 |
| 中继不通 | VPS unit 是否有 `--relay-addr 47.105.103.246`;`ss -ulnp | grep 8788`;安全组 UDP 8788;账号 `relay enable` |
| 传输失败无角标 | 对端在线态;`mtdss ls 对端:` 排除 ctrl 面问题;T5 未过则中继兜底缺件 |
| MIX 3 装机失败 | `adb install -r` 证书不符 → 先 `adb uninstall com.mtdss.android`(清数据,需重登) |
| WSL 脚本路径怪 | `MSYS_NO_PATHCONV=1 wsl.exe bash /mnt/c/...`(Git Bash 路径改写坑) |
