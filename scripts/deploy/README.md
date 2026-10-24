# MTDSS 服务端部署资产(VPS)

本目录是生产部署的最小资产集:`Caddyfile`(443 统一入口)+ 两个 systemd unit(信令 / STUN)。
配套文档:发布流程见 `.github/workflows/release.yml`(Task 18,tag `v*` 驱动),发布演练步骤见
`docs/smoke-m4.md`(Task 19)。更新/验签机制本身见 `crates/daemon/src/update.rs` 与 `mtdss-sign`。

## 目录布局

VPS 上的约定路径(与仓内文件一一对应):

| 路径 | 用途 |
| --- | --- |
| `/usr/local/bin/signaling-server` | 信令二进制(`cargo build --release -p signaling` 产物) |
| `/usr/local/bin/mtdss-admin` | 运维账号 CLI(同上,与 signaling-server 同包产物) |
| `/usr/local/bin/stun-server` | STUN 二进制(`cargo build --release -p stun-server` 产物) |
| `/etc/mtdss/tls/server.crt` `/etc/mtdss/tls/server.key` | 443 TLS 证书/私钥(Caddyfile 引用) |
| `/etc/mtdss/tls/ca.crt` | 自签 CA(仅备案前演练期需要) |
| `/var/lib/mtdss/signaling.db` | 信令 SQLite(unit `StateDirectory=mtdss` 自动建目录) |
| `/var/www/mtdss/dl/` | 更新分发根(Caddy `/dl/*` 静态目录,见下方「更新发布目录规约」) |
| `/etc/systemd/system/mtdss-signaling.service` | 本目录 unit 拷入(替换 `<公网IP>` 后) |
| `/etc/systemd/system/mtdss-stun.service` | 本目录 unit 拷入 |
| `/etc/caddy/Caddyfile` | 本目录 Caddyfile 拷入(证书路径按实际替换) |

## 首次部署

1. 构建:`cargo build --release -p signaling -p stun-server`,上传两二进制到
   `/usr/local/bin/` 并 `chmod +x`(也可直接用 release workflow 的产物)。
2. 建系统用户:`useradd -r -s /usr/sbin/nologin mtdss`。
3. 拷贝两个 unit 与 Caddyfile(见上表);`mtdss-signaling.service` 内 `<公网IP>` 必须替换。
4. 证书:域名就绪前先走「备案前演练拓扑」自签;就绪后换成正式签发的 crt/key 放 `/etc/mtdss/tls/`。
5. 防火墙:放行 **TCP 443**、**UDP 3478/3479** 与 **UDP 8788**(中继,见下节);`8787` 仅回环,严禁对外开放。
6. 启动:`systemctl daemon-reload && systemctl enable --now mtdss-stun mtdss-signaling caddy`。
7. 验证(本机):
   `curl -s -X POST https://127.0.0.1/auth/request-code -H 'content-type: application/json' -d '{"email":"ops@example.com"}'`
   返回 200/JSON 即 Caddy→信令链路通(`ops@example.com` 须已预置,见下节「账号预置」;
   未预置邮箱一律 404);STUN 用 `mtdss` 客户端 NAT 探测或抓包 3478 验证。

## 账号预置(mtdss-admin)

v2 起登录收口到预置账号:**先建号才能登录**——`/auth/request-code` 对未预置邮箱一律
404,ws 握手对未认证设备同样拒绝。二进制与 signaling-server 同包
(`cargo build --release -p signaling` 同时产出),拷到 `/usr/local/bin/mtdss-admin`。语法:

```
mtdss-admin [--db <path>] user add <email> | user remove <email> | user list [--with-usage]
mtdss-admin [--db <path>] relay enable <email> | relay disable <email> | relay limit <email> <kbps> | relay quota <email> <GB>
```

| 命令 | 作用 |
| --- | --- |
| `user add <email>` | 预置账号(幂等,已存在则原样保留既有配置);新账号中继默认 **disabled** |
| `user remove <email>` | 删除账号并级联清理 devices/codes/relay_config/relay_usage(单事务;不存在报错) |
| `user list [--with-usage]` | 全量账号清单(按 email 排序);`--with-usage` 追加当期用量列(字节),默认不显 |
| `relay enable <email>` / `relay disable <email>` | 开/关该账号的中继能力(默认关闭) |
| `relay limit <email> <kbps>` | 中继限速(kbps),0=不限,默认 1024 |
| `relay quota <email> <GB>` | 月度流量配额(GiB),0=不限,默认 10 |

- `--db` 默认 `signaling.db`(与 server 默认一致);生产上指 `--db /var/lib/mtdss/signaling.db`。
- **运行属主**:服务以 `User=mtdss` 运行,首启前建号同样要以该用户执行——root 直跑会
  留下 root 属主的 db,服务 crash-loop。规范形如:
  `sudo -u mtdss mtdss-admin --db /var/lib/mtdss/signaling.db user add <email>`。
- **WAL 并存**:库为 WAL 模式,signaling-server 运行中可直接执行本 CLI,无需停服。
- **生产遗留账号**:v2 之前已存在、缺 relay_config 行的账号,`user list` 按
  disabled + 默认限速/配额(1024 kbps / 10 GiB)呈现;任一 `relay` 子命令首次触达
  会按默认值自动补建行(仍 disabled),既有配置不会被被动改动。

## 中继数据面(UDP 8788)

signaling-server 内置中继数据面,与信令同进程默认启动:`--relay-port <p>` 绑定端口
(默认 8788,单 UDP 端口哑转发,无对应 TCP 口),`--relay-addr <host>` 对外通告地址
(默认 127.0.0.1,**生产须与 `--stun-addr` 一样换成公网 IP**,unit 模板已预置占位符;
该地址随分配帧下发给客户端,客户端按它直连中继)。

- **安全组/防火墙**:放行 **UDP 8788**(换 `--relay-port` 时同步改)。
- **全局带宽上限** `MTDSS_RELAY_GLOBAL_KBPS`(env,可选):全部会话共享的总上限,
  按 kbps 计,默认 2500;坏值回退默认并打 warn;`0` = 不限。unit 中以 `Environment=`
  注入或保持默认(模板内有注释行示例)。
- **运营三步**(`mtdss-admin`,生产记得 `sudo -u mtdss` + `--db /var/lib/mtdss/signaling.db`,详见上节):
  1. `user add <email>` — 预置账号(新账号中继默认 disabled);
  2. `relay enable <email>` — 开通中继;
  3. `relay quota <email> <GB>` / `relay limit <email> <kbps>` — 按需调月度配额/限速
     (默认 10 GiB / 1024 kbps;`0` = 不限)。
- **客户端行为**(v2-P1c 起):客户端打洞穷尽即自动走中继兜底,无需任何客户端
  配置;远端浏览(`mtdss ls`)等经 ctrl 通道的 RPC 同样自动兜底(v2-P1c 第二刀
  起);账号未 `relay enable` 则兜底不可用,打洞穷尽按传输失败收束。

## 备案前演练拓扑(域名未就绪,纯 IP + 自签 CA)

目标:无域名/无备案时,在 VPS IP 上跑通「登录 + 更新全链」,供 Task 19 发布演练(①-⑤)使用。

1. 生成带 IP SAN 的自签证书(VPS 上):
   `signaling-server --dev-gen-tls /etc/mtdss/tls --tls-san <VPS_IP>`
   (`--tls-san` 可重复注入多个 IP/域名;生成的 `server.crt` 已含 `localhost/127.0.0.1/::1` 与注入项)
   拷 `ca.crt` 留作客户端信任根,Caddyfile 的 `tls` 行指向生成的 crt/key。
2. daemon/CLI 侧(演练真机):
   - `MTDSS_UPDATE_URL=https://<VPS_IP>/dl` — 更新基址走 IP;
   - `MTDSS_UPDATE_CA=<路径>/ca.crt` — 信任自签 CA(运行时 env,`crates/daemon/src/update.rs` 读取);
   - 构建期 `MTDSS_UPDATE_PUBKEY=<公钥 hex>` 照常注入,验签不受演练影响。
3. 演练期发布产物照常放 `/var/www/mtdss/dl/`(见下节),`manifest.json` 的 URL 形态为
   `https://<VPS_IP>/dl/win/<ver>/<file>`,与 daemon 侧 `MTDSS_UPDATE_URL` 同基址。

## 域名就绪切换步骤与回退

切换(假设新域名为 `mtdss.example.com`):

1. 签发证书并替换 `/etc/mtdss/tls/{server.crt,server.key}`(certbot/acme.sh DNS 验证均可),
   `caddy validate --config /etc/caddy/Caddyfile && systemctl reload caddy`;
   可选:把站点地址 `:443` 改为 `<域名>:443` 收紧 SNI。
2. 客户端基址切域名:构建期注入 `MTDSS_UPDATE_URL=https://mtdss.example.com/dl`
   (见 `mtdss_ipc::consts::DEFAULT_UPDATE_URL`),重新走 Task 18 workflow 发布;
   新版 manifest 的 `url_base` 即新域名。
3. 过渡期旧 IP 形态保持可达(旧版客户端仍按旧 URL 拉 manifest/安装包),
   观察一个发布周期后再回收。

回退:

- 服务端:恢复 `/etc/mtdss/tls/` 下旧证书(或演练期自签形态)+ `systemctl reload caddy`。
- 客户端:`mtdss update <旧版本>` 版本钉扎回滚(smoke-m4 测试项 D);
  自签 CA 演练形态与域名形态仅差 env 与证书文件,回退即换回对应组合。

## 更新发布目录规约

```
/var/www/mtdss/dl/
├── manifest.json                     # 最新版签名清单(release workflow publish 或手工 mtdss-sign 上传)
└── win/
    └── <ver>/                        # 如 win/1.0.1/,按版本沉淀,供回滚下载旧安装包
        ├── MTDSS_<ver>_x64-setup.exe # NSIS 安装器(tauri 产物命名,与 manifest 内 url 一致)
        └── manifest.json             # 版本钉扎位(release publish 同步上传;`mtdss update <ver>` 回滚/重装本版)
```

- `manifest.json` 恒指向「最新可升级版本」;清单内 asset URL 形如
  `<url_base>/win/<ver>/<文件名>`(`mtdss-sign manifest` 生成,见 Task 18 build-windows 步)。
- 手工发布:`mtdss-sign manifest --version <ver> --url-base <base> --notes-file <f> --file <setup.exe> --out unsigned.json`
  → `mtdss-sign sign` → `mtdss-sign verify` → 上传 `manifest.json` 与安装包到对应目录。
- Linux/macOS tar 包(`mtdss-<ver>-<x86_64-linux|aarch64-macos>.tar.gz`,Task 17
  `scripts/package/package-tar.sh` 产物)**无自动更新能力**,不进 `/dl` 目录,直接附 GH Release。
