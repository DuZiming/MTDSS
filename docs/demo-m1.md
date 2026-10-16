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
cargo run -p signaling -- signaling.db
```

## 设备 A 注册（终端 2）
```bash
cargo run -p mtdss-cli -- register --server http://127.0.0.1:8787 --email a@x.y --name home-pc
# 按提示输入 dev-mode 验证码
```

## 设备 B 注册（终端 3，同一邮箱）
```bash
$env:MTDSS_HOME = "$PWD\.dev-b"   # Windows；Linux/macOS: export MTDSS_HOME=$PWD/.dev-b
cargo run -p mtdss-cli -- register --server http://127.0.0.1:8787 --email a@x.y --name office-pc
```

## B 接收（终端 3）
```bash
cargo run -p mtdss-cli -- recv --out ./inbox-b
```

## A 发送（终端 2）
```bash
cargo run -p mtdss-cli -- devices
cargo run -p mtdss-cli -- send ./任意文件.zip --to <B 的 device_id>
```

预期：B 的 inbox-b 出现文件，A 显示 sent N bytes。
