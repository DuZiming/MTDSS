# M1 演示：两台设备经信令服务器互传文件

## 前置条件（Prerequisites）

- Rust stable（[rustup](https://rustup.rs)）
- `protoc` 必须在 PATH 上（或设置 `PROTOC` 环境变量指向其完整路径）——
  `proto` crate 的 build script 依赖它，缺失时构建会报错
  - Windows: `winget install Google.Protobuf`
  - Debian/Ubuntu: `sudo apt install protobuf-compiler`
  - macOS: `brew install protobuf`

## 起服务器（终端 1）
```bash
# 一次性生成自签 CA 与证书（目录可任意，勿提交入库）
cargo run -p signaling -- --dev-gen-tls .dev-tls
cargo run -p signaling -- signaling.db --tls-cert .dev-tls/server.crt --tls-key .dev-tls/server.key
```

## （可选）起 STUN（终端 1b，跨公网直连需要）
```bash
cargo run -p stun-server   # 默认 0.0.0.0:3478/:3479
```
信令服务器通告：`--stun-addr <公网IP> --stun-port 3478,3479`（默认 127.0.0.1，仅本机演示）。

## 设备 A:起守护进程(终端 2)+ 登录(终端 2')
```bash
# 终端 2:常驻守护进程(信令在线循环与常驻接收都在它身上)
cargo run -p mtdss-daemon --bin mtdssd -- --recv-dir ./inbox-a
```
```bash
# 终端 2':登录——先回显 dev 验证码,按提示输入完成验证
cargo run -p mtdss-cli -- login --server https://127.0.0.1:8787 --ca .dev-tls/ca.crt --email a@x.y --name home-pc
```

## 设备 B:起守护进程(终端 3)+ 登录(终端 3',同一邮箱)
```bash
$env:MTDSS_HOME = "$PWD\.dev-b"   # Windows;Linux/macOS: export MTDSS_HOME=$PWD/.dev-b
cargo run -p mtdss-daemon --bin mtdssd -- --recv-dir ./inbox-b
```
```bash
$env:MTDSS_HOME = "$PWD\.dev-b"
cargo run -p mtdss-cli -- login --server https://127.0.0.1:8787 --ca .dev-tls/ca.crt --email a@x.y --name office-pc
```

## A 发送(终端 2')
```bash
cargo run -p mtdss-cli -- devices
cargo run -p mtdss-cli -- send <文件或目录...> --to <B 的 device_id>
```

可一次发送多个文件或目录，目录会递归展开并在 B 侧还原目录结构。
传输中断（网络抖动 / 进程被杀）后，重发同一批输入即可续传：对端已完整
持有的文件自动跳过，只补收未完成文件的缺失块。

预期：B 的 inbox-b 出现文件（目录结构保持），A 显示 sent N bytes。
