# Mac mini 构建盒通道手册(V2-P5)

Mac mini(Apple Silicon,macOS 26.3,Xcode 26.6)与 Windows 开发机网络不通,经 VPS(47.105.103.246)反向隧道桥接。本文是唯一权威用法手册。

## 拓扑

```
Windows ──ssh──▶ VPS(47.105.103.246)◀──autossh -R 2222── Mac mini(kk@)
                    │ ProxyJump                        ~/mtdss-src.git(bare 中转)
                    └──────────────── localhost:2222 ──▶ Mac sshd
```

- Mac 侧常驻:`~/Library/LaunchAgents/top.mtdss.tunnel.plist`(autossh,KeepAlive 自愈,断电重启后 RunAtLoad 自动拉起)
- 密钥:Mac→VPS 用 `~/.ssh/mtdss_tunnel_id`;Windows→VPS、Windows→Mac 用 Windows `~/.ssh/id_ed25519`
- Windows 侧 `~/.ssh/config` 已配 `Host mac`(ProxyJump root@VPS,Port 2222,User kk)

## 环境文件(必读)

Mac 上跑任何构建命令前 `source ~/mtdss-env.sh`:
- `DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer`——**xcode-select 指向 CommandLineTools 的坑由此绕开**(无需 sudo)
- PATH 带 /opt/homebrew/bin 与 ~/.cargo/bin

永久修(可选,一次性,需密码):`sudo xcode-select -s /Applications/Xcode.app/Contents/Developer`

## 代码同步(Mac 不连 GitHub,国内网络;全部走隧道)

```
# Windows(worktree 内):
git push macmini worktree-worktree-mtdss-v2p5-ios
# Mac:
cd ~/mtdss-ios && git fetch -q origin && git reset -q --hard origin/worktree-worktree-mtdss-v2p5-ios
```
remote `macmini` = `mac:~/mtdss-src.git`(bare);`~/mtdss-ios` 的 origin 指向该 bare。

## 常用命令

| 事 | 命令(Mac 上,先 source ~/mtdss-env.sh) |
|---|---|
| 引擎 iOS 静态库 | `cargo build -p ios-bridge --target aarch64-apple-ios --release`(产物 `target/aarch64-apple-ios/release/libios_bridge.a`,首编约 26s 增量/全量约十几分钟) |
| Swift 包测试 | `cd ios/package && swift test` |
| Xcode 工程生成 | `cd ios/app && xcodegen generate` |
| App 构建(模拟器) | `ios/build.sh sim` |
| 模拟器截图 | `xcrun simctl io <udid> screenshot out.png`(配合 `xcrun simctl list` 找 udid;iOS 26.5 runtime 已装) |
| iOS 平台包 | 已装(iOS 26.5);重装:`xcodebuild -downloadPlatform iOS` |

依赖:brew(protobuf[prost-build 需要]/xcodegen/autossh)、rustup targets(aarch64-apple-ios、aarch64-apple-darwin)。

## 故障排查

| 症状 | 处置 |
|---|---|
| ssh 报 Connection reset(VPS sshd 偶发) | 重试即过(已配 ServerAliveInterval=15) |
| 隧道断(2222 不在 VPS 上听) | Mac 上 `launchctl kickstart -k gui/$(id -u)/top.mtdss.tunnel`;仍不通查 Mac 出网 |
| xcodebuild 报 CLT 错 | 忘了 source ~/mtdss-env.sh |
| cargo 报 protoc 缺失 | 同上(env 里 PATH 含 brew) |
| GitHub 直连超时 | 预期行为,走 macmini bare 中转,勿改 |

## 一次性安装账(2026-10-31,已完成)

用户物理安装(Xcode 26/系统/远程登录/断电自启/rustup/brew/autossh/密钥)→VPS 公钥配置→隧道 LaunchAgent→Windows Host mac→xcodegen 2.46/rust aarch64-apple-ios/protobuf 安装→iOS 26.5 平台包下载→T2 libios_bridge.a 交叉编译验证通过。
