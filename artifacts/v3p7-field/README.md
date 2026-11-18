# V3-7 测试包(票池修复波,2026-11-17)

- **Win** `Synshu_1.3.0-v3p7-x64-setup.exe`(6.62 MiB)
  sha256 `40bd5753028a2d17fe586c7e813ce28221b120aea13730286f5b3b2f1177226c`
  基座 main@b3817f0(引擎树 = PR #31 merge f043270 @3c706cb + docs);
  内嵌 mtdssd `37f12ff2…`(sidecar 与 target/release 逐字节一致);
  更新链双串烤入各 1 命中(URL www.mtdss.top/dl/synshu + pubkey)。
- 服务端:VPS 未动(旧服双向兼容;rendezvous 待下波部署激活,不影响本包测试)。
- smoke 重点:并发传输同名文件(本波主战场)/收件箱子目录轮/任务列表长跑。
- **Linux x86_64** `mtdss-1.3.0-v3p7-x86_64-linux.tar.gz`(9.2 MB)
  sha256 `224f6386debb3fa44f14ce14f5c20cba4b9d8dfc2aacec9e6542f4edcd9b809c`
  (WSL 原生构建 main@b3817f0;域名烤入 1/1;.invalid 0;更新通道 0=tar 设计态;
  V3-7 引擎指纹 rendezvous 8 命中;安装/服务面见 docs/manual-linux.md)

## r2(smoke 四票跟进波,2026-11-18,源=v3p7-followup @77acf97 = PR #32 六绿树)

- **Win** `Synshu_1.3.0-v3p7r2-x64-setup.exe`(6.95 MiB)
  sha256 `fc09b9fa5d0552a5b9f086905d2c02d431de69ad3dcec2be9531500fa5979ac1`
  (内嵌 mtdssd `feb9ccfe…` sidecar 与 target/release 逐字节一致;更新链双串各 1 命中)
- **Linux x86_64** `mtdss-1.3.0-v3p7r2-x86_64-linux.tar.gz`(9.2 MB)
  sha256 `b33700d7ef3d70346d9fce62a6e4a50912c99490b7a6a6c4119d102565e259a1`
  (域名烤入 1/1;.invalid 0;更新通道 0=tar 设计态;rendezvous 指纹 8 命中)
- 四票:④继续钮 SVG/⑤发送侧 Resume 种子(重发进度自断点)/⑥终态冻结条+悬停显错/⑦清除收件箱形态路由

## r3(⑤ UX 收口:重发显示下限,2026-11-18,源=v3p7-followup @ea97261)

- **Win** `Synshu_1.3.0-v3p7r3-x64-setup.exe` sha256 `d7bb7ea2cc3743ab02e3b98e0a96134252f2ea0a55ba638fcb87b6018cd4c265`
- **Linux** `mtdss-1.3.0-v3p7r3-x86_64-linux.tar.gz` sha256 `7f689f8e6b2d19e6d15f369c762db517503070b4c47d0b3d733fd7c1887a2cfa`
- **Android** `Synshu-v3p7r3-debug.apk` sha256 `c884e7c53221a8ed7e9bac93dd42d389ad8a2c7693c5796b96ec0f047e6583ad`
- 增量:r2 全部 + 重发显示下限(扫描期即显断点位);CI 六绿(run 重跑,一轮 windows 老家族单抽重跑绿)
