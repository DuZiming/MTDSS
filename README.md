# MTDSS(Synshu)

跨设备文件直传/预览引擎与客户端。各波的 spec/冒烟手册见 `docs/`;构建与
CI 口径见 `.github/workflows/` 与 `.cargo/config.toml` 内注释。

## HEIC 预览转码(libheif)分发注记

桌面端(仅 Windows)的 HEIC→JPEG 预览转码基于第三方库
[libheif](https://github.com/strukturag/libheif)(LGPL-2.1 及其后续版本)
与 [libde265](https://github.com/strukturag/libde265)(LGPL-3.0 及其后续
版本),以**运行时动态加载**方式消费(`LoadLibrary`,不进链接期依赖):

- **动态链即 LGPL 合规面**:libheif/libde265 以独立 DLL(`libheif.dll`、
  `libde265.dll` 等)随安装包分发,与本体进程隔离,用户可自行替换或移除
  同名 DLL;LGPL 要求的「库可换、源可得」据此满足。
- **源码获取**:libheif 与 libde265 的源码在其上游仓库(上述 GitHub 链接
  )公开;本仓库不含其修改版(原样使用)。
- **打包 DLL 不含 x265**:解码用构建须去掉 HEVC 编码器特性(vcpkg 口径
  `vcpkg install "libheif[core,libde265]"` 或装后剔除 x265 依赖链),
  x265 为 GPL-2.0+,仅解码面不需要它——打包带入会造成 GPL 传染。x265
  只用于**开发/CI 测试环境**(单元测试用 libheif 自带编码器现编 HEIC
  夹具,`vcpkg install libheif:x64-windows` 默认含 x265),不出包。
- **部署位置**:DLL 与 `mtdssd.exe` 同目录(标准 DLL 搜索序第一顺位);
  DLL 缺失时 HEIC 转码面诚实降级(HTTP 503,客户端回退 raw),其余预览
  面不受影响。
- **依赖 DLL 的 CRT 运行库**(vcruntime140.dll 等)随包一并分发,与
  本体 `+crt-static`(免管理员安装不预装 VC++ 运行库)的口径互不影响
  ——本体不依赖这些运行库,只有 libheif.dll 自己需要。
