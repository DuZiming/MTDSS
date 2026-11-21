//! 桌面 HEIC→JPEG 转码(V4-1 T6):仅 Windows(`heic` feature 门,默认关;
//! 非 Windows / 未启用 = loopback /jpeg 维持 404 占位,客户端回退 raw)。
//!
//! ## 选型与合规(README「HEIC 预览转码(libheif)分发注记」同源)
//!
//! libheif 以**运行时动态加载**消费(`LoadLibraryW` + `GetProcAddress`,
//! 首次解码装载后进程常驻不卸载):链接期零 C 依赖,仓库构建不引入任何
//! 外部工具链(vcpkg/pkg-config/cmake 全不沾)。不走 `libheif-sys` 绑定
//! crate 的原因(2026-11 选型实证):其 Windows build.rs 对 vcpkg crate
//! 硬依赖(无 vcpkg 即 `exit(1)`),而 vcpkg crate 在本仓库的 Windows 构
//! 建形态(`.cargo/config.toml` 钉 `+crt-static`,免管理员分发不依赖 VC++
//! 运行库)下强制选 `x64-windows-static` 三元组——纯静态链,LGPL 静态链
//! 合规负担(须随包提供目标文件重链面),与 brief 钉死的「动态链」相抵;
//! `embedded-libheif` 特性仅 Linux 且同为静态。运行时加载 = 纯动态链
//! (LGPL 合规面:用户可替换 DLL)+ 零构建脆面 + 缺 DLL 诚实降级
//! ([`DecodeError::Unavailable`] → 回环口 503)。
//!
//! DLL 探测序:[`DLL_CANDIDATES`] 逐名 `LoadLibraryW`(标准搜索序 = exe
//! 目录 → 系统 → PATH;部署面把 libheif.dll 置 mtdssd.exe 同目录即达)。
//! 任一符号缺失视为版本不符 = 整体不可用(拒绝半加载态)。
//!
//! 嗅探(brief「非 HEIC→404」):ISO BMFF `ftyp` 主品牌面白名单(静态图
//! 片品牌 `heic`/`heix`/`heim`/`heis`/`mif1`;序列品牌 `hevc`/`hevx` 不
//! 收——视频面归 T15;`avif` 不收——打包 DLL 无 AV1 解码器,收了也只能
//! 422,不如诚实 404 让客户端回退 raw)。嗅探先于解码,HEIC 外源零接触
//! libheif。
//!
//! 解码:主图 → RGB 交错逐行取面(stride 诚实按行拷贝)→ 纯 Rust JPEG
//! 编码(`jpeg-encoder`,质量 90)。面 guards:像素总量 ≤
//! [`MAX_PIXELS`](远控文件内存炸弹面,100MP ≈ 300MB RGB 上界)、宽高
//! ≤ 65535(JPEG 尺寸域)。全部调用点 spawn_blocking(调用方责任)。
//!
//! 测试面(自足往返,免外部二进制夹具):[`encode_test_heic`] 用 libheif
//! 自带 HEVC 编码器(x265 插件)**现编**夹具 HEIC——本机/CI 的测试 DLL 须
//! 带 x265(`vcpkg install libheif:x64-windows` 默认含);**打包 DLL 不带
//! x265**(仅解码,规避 GPL 传染,见 README)。解码计数器
//! [`DECODE_CALLS`] 与串行锁 [`TEST_SERIAL`] 供回环口「缓存命中零重解码」
//! 断言(并行测试下增量不失真)。

#![cfg(all(windows, feature = "heic"))]

use std::ffi::{c_char, c_void, CStr};
use std::sync::OnceLock;

use windows_sys::core::{PCSTR, PCWSTR};
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

/// JPEG 编码质量(预览面:足够锐,不追极致;0-100)。
const JPEG_QUALITY: u8 = 90;
/// 像素总量帽(远控文件的内存炸弹面;100MP × 3B ≈ 300MB RGB 上界,
/// 手机/相机照片 ≤ ~50MP,余量充足)。超帽按解码失败(422)拒。
const MAX_PIXELS: u64 = 100 * 1000 * 1000;
/// JPEG 编码尺寸域上限(u16;libheif 侧 HEIC 极少越界,越界 = 422)。
const MAX_DIM: u64 = u16::MAX as u64;

/// DLL 探测名序(vcpkg 动态三元组建 `heif.dll`;MinGW 常名 `libheif-1.dll`
/// ;MSYS/通用别名 `libheif.dll`)。
const DLL_CANDIDATES: [&str; 3] = ["libheif.dll", "heif.dll", "libheif-1.dll"];

// ---------- libheif C API 面(与官方 heif.h 对齐;枚举常量锚定
// libheif-sys v1_23 预生成绑定,1.17+ 全程稳定) ----------

type HeifContext = *mut c_void;
type HeifImageHandle = *mut c_void;
type HeifImage = *mut c_void;
type HeifEncoder = *mut c_void;

/// `struct heif_error { heif_error_code code; heif_suberror_code subcode;
/// const char* message; }`(两枚举 = C int;message 恒非 NULL,头文件口径)。
#[repr(C)]
struct HeifError {
    code: i32,
    subcode: i32,
    message: *const c_char,
}

const HEIF_ERROR_OK: i32 = 0;
const COLORSPACE_RGB: i32 = 1; // heif_colorspace_RGB
const CHROMA_INTERLEAVED_RGB: i32 = 10; // heif_chroma_interleaved_RGB
const CHANNEL_INTERLEAVED: i32 = 10; // heif_channel_interleaved
                                     // 测试夹具编码面专用(lib 构建不消费;见下方 HeifApi 编码字段同口径)。
#[cfg_attr(not(test), allow(dead_code))]
const COMPRESSION_HEVC: i32 = 1; // heif_compression_HEVC

/// C 函数指针类型缩写:`heif_fn!(A, B => R)` = `unsafe extern "C" fn(A, B) -> R`。
macro_rules! heif_fn {
    ($($arg:ty),* $(,)? => $ret:ty) => {
        unsafe extern "C" fn($($arg,)* ) -> $ret
    };
}

/// 逐符号解析出的 libheif 函数表(进程常驻;fn 指针天然 Send+Sync)。
struct HeifApi {
    context_alloc: heif_fn!(=> HeifContext),
    context_free: heif_fn!(HeifContext => ()),
    read_from_memory: heif_fn!(HeifContext, *const c_void, usize, *const c_void => HeifError),
    get_primary_image_handle: heif_fn!(HeifContext, *mut HeifImageHandle => HeifError),
    image_handle_get_width: heif_fn!(HeifImageHandle => i32),
    image_handle_get_height: heif_fn!(HeifImageHandle => i32),
    decode_image: heif_fn!(
        HeifImageHandle, *mut HeifImage, i32, i32, *const c_void => HeifError
    ),
    image_get_plane_readonly2: heif_fn!(HeifImage, i32, *mut usize => *const u8),
    image_handle_release: heif_fn!(HeifImageHandle => ()),
    image_release: heif_fn!(HeifImage => ()),
    // —— 以下仅测试夹具编码面消费(打包 DLL 无 x265 时解析仍成立,只是
    // 拿不到 HEVC 编码器;decode 面不依赖)——
    #[cfg_attr(not(test), allow(dead_code))]
    image_create: heif_fn!(i32, i32, i32, i32, *mut HeifImage => HeifError),
    #[cfg_attr(not(test), allow(dead_code))]
    image_add_plane: heif_fn!(HeifImage, i32, i32, i32, i32 => HeifError),
    #[cfg_attr(not(test), allow(dead_code))]
    image_get_plane2: heif_fn!(HeifImage, i32, *mut usize => *mut u8),
    #[cfg_attr(not(test), allow(dead_code))]
    get_encoder_for_format: heif_fn!(HeifContext, i32, *mut HeifEncoder => HeifError),
    #[cfg_attr(not(test), allow(dead_code))]
    encoder_set_lossy_quality: heif_fn!(HeifEncoder, i32 => HeifError),
    #[cfg_attr(not(test), allow(dead_code))]
    context_encode_image: heif_fn!(
        HeifContext, HeifImage, HeifEncoder, *const c_void, *mut HeifImageHandle => HeifError
    ),
    #[cfg_attr(not(test), allow(dead_code))]
    encoder_release: heif_fn!(HeifEncoder => ()),
    #[cfg_attr(not(test), allow(dead_code))]
    context_write_to_file: heif_fn!(HeifContext, *const c_char => HeifError),
}

static API: OnceLock<HeifApi> = OnceLock::new();

/// 函数表装载(缺 DLL / 缺符号 = None;幂等可并发,末位 set 胜)。
fn load_api() -> Option<HeifApi> {
    let module: HMODULE = DLL_CANDIDATES.iter().find_map(|name| {
        let mut wide: Vec<u16> = name.encode_utf16().collect();
        wide.push(0);
        let m = unsafe { LoadLibraryW(wide.as_ptr() as PCWSTR) };
        (!m.is_null()).then_some(m)
    })?;
    // 逐符号解析;任一缺失 = DLL 与本表面不符(版本过老),整体不可用。
    // 卸载面:进程常驻不 FreeLibrary(daemon 单份消费,卸载语义无收益,
    // 免在途调用与析构序面)。transmute 双类型显式注记(FARPROC 内型 →
    // 具体签名;ABI 契约 = heif.h 签名逐一比对,模块注「C API 面」)。
    macro_rules! sym {
        ($name:literal: $ty:ty) => {{
            let p: PCSTR = concat!($name, "\0").as_ptr();
            match unsafe { GetProcAddress(module, p) } {
                Some(f) => unsafe {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>(f)
                },
                None => {
                    tracing::warn!(dll = %DLL_CANDIDATES[0], "libheif 符号缺失(DLL 版本不符),转码面不可用");
                    return None;
                }
            }
        }};
    }
    Some(HeifApi {
        context_alloc: sym!("heif_context_alloc": heif_fn!(=> HeifContext)),
        context_free: sym!("heif_context_free": heif_fn!(HeifContext => ())),
        read_from_memory: sym!(
            "heif_context_read_from_memory":
            heif_fn!(HeifContext, *const c_void, usize, *const c_void => HeifError)
        ),
        get_primary_image_handle: sym!(
            "heif_context_get_primary_image_handle":
            heif_fn!(HeifContext, *mut HeifImageHandle => HeifError)
        ),
        image_handle_get_width: sym!("heif_image_handle_get_width": heif_fn!(HeifImageHandle => i32)),
        image_handle_get_height: sym!("heif_image_handle_get_height": heif_fn!(HeifImageHandle => i32)),
        decode_image: sym!(
            "heif_decode_image":
            heif_fn!(HeifImageHandle, *mut HeifImage, i32, i32, *const c_void => HeifError)
        ),
        image_get_plane_readonly2: sym!(
            "heif_image_get_plane_readonly2":
            heif_fn!(HeifImage, i32, *mut usize => *const u8)
        ),
        image_handle_release: sym!("heif_image_handle_release": heif_fn!(HeifImageHandle => ())),
        image_release: sym!("heif_image_release": heif_fn!(HeifImage => ())),
        image_create: sym!(
            "heif_image_create": heif_fn!(i32, i32, i32, i32, *mut HeifImage => HeifError)
        ),
        image_add_plane: sym!(
            "heif_image_add_plane": heif_fn!(HeifImage, i32, i32, i32, i32 => HeifError)
        ),
        image_get_plane2: sym!(
            "heif_image_get_plane2": heif_fn!(HeifImage, i32, *mut usize => *mut u8)
        ),
        get_encoder_for_format: sym!(
            "heif_context_get_encoder_for_format":
            heif_fn!(HeifContext, i32, *mut HeifEncoder => HeifError)
        ),
        encoder_set_lossy_quality: sym!(
            "heif_encoder_set_lossy_quality": heif_fn!(HeifEncoder, i32 => HeifError)
        ),
        context_encode_image: sym!(
            "heif_context_encode_image":
            heif_fn!(HeifContext, HeifImage, HeifEncoder, *const c_void, *mut HeifImageHandle => HeifError)
        ),
        encoder_release: sym!("heif_encoder_release": heif_fn!(HeifEncoder => ())),
        context_write_to_file: sym!(
            "heif_context_write_to_file": heif_fn!(HeifContext, *const c_char => HeifError)
        ),
    })
}

/// 函数表就绪(缺 DLL = [`DecodeError::Unavailable`] → 回环口 503)。
fn api() -> Result<&'static HeifApi, DecodeError> {
    if let Some(a) = API.get() {
        return Ok(a);
    }
    match load_api() {
        Some(a) => {
            let _ = API.set(a);
            Ok(API.get().expect("刚 set 必在"))
        }
        None => Err(DecodeError::Unavailable),
    }
}

// ---------- 解码面 ----------

/// ftyp 品牌嗅探(brief「非 HEIC→404」;静态图片品牌白名单,见模块注
/// 「嗅探」):ISO BMFF 头 `len(4) "ftyp" brand(4)`,主品牌落白名单即 HEIC。
pub(crate) fn sniff_heic(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && &bytes[4..8] == b"ftyp"
        && matches!(
            &bytes[8..12],
            b"heic" | b"heix" | b"heim" | b"heis" | b"mif1"
        )
}

/// 转码失败二态:运行库缺席(部署面,503)与解码失败(源面,422)。
#[derive(Debug)]
pub(crate) enum DecodeError {
    /// libheif.dll 未部署 / 符号不符(DLL 缺失 = 部署面问题,客户端重试
    /// 无益但服务仍活——回环口映 503,客户端回退 raw)。
    Unavailable,
    /// 源确为 HEIC 但解码失败(损坏/越界/插件缺)——回环口映 422。
    Bad(String),
}

/// 解码计数(仅测试面:回环口「缓存命中零重解码」断言锚)。
#[cfg(test)]
pub(crate) static DECODE_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// 测试串行锁(并行测试下解码计数增量不失真:凡动解码的用例全程持锁)。
/// tokio Mutex(回环口用例跨 await 持锁;`await_holding_lock` 面净)——
/// 纯同步用例用 `blocking_lock`(无运行时上下文,合法)。
#[cfg(test)]
pub(crate) static TEST_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// HEIC 字节流 → JPEG 字节流(主图;RGBA 源转 RGB 时丢弃 alpha,预览面
/// 无损于展示)。失败二态见 [`DecodeError`]。
pub(crate) fn decode_heic_to_jpeg(heic: &[u8]) -> Result<Vec<u8>, DecodeError> {
    #[cfg(test)]
    DECODE_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let api = api()?;
    unsafe {
        let ctx = (api.context_alloc)();
        if ctx.is_null() {
            return Err(DecodeError::Bad("heif_context_alloc 失败".into()));
        }
        let decoded = decode_with(api, ctx, heic);
        (api.context_free)(ctx);
        decoded
    }
}

/// 解码主体(ctx 所有权归调用方;本函数只管读→解码→编码的干净收放)。
unsafe fn decode_with(
    api: &HeifApi,
    ctx: HeifContext,
    heic: &[u8],
) -> Result<Vec<u8>, DecodeError> {
    let err = (api.read_from_memory)(
        ctx,
        heic.as_ptr() as *const c_void,
        heic.len(),
        std::ptr::null(),
    );
    if err.code != HEIF_ERROR_OK {
        return Err(DecodeError::Bad(format!(
            "HEIF 解析失败: {}",
            heif_msg(&err)
        )));
    }
    let mut handle: HeifImageHandle = std::ptr::null_mut();
    let err = (api.get_primary_image_handle)(ctx, &mut handle);
    if err.code != HEIF_ERROR_OK {
        return Err(DecodeError::Bad(format!("取主图失败: {}", heif_msg(&err))));
    }
    let (w, h) = (
        (api.image_handle_get_width)(handle),
        (api.image_handle_get_height)(handle),
    );
    if w <= 0 || h <= 0 {
        (api.image_handle_release)(handle);
        return Err(DecodeError::Bad(format!("非法图尺寸 {w}x{h}")));
    }
    let (w64, h64) = (w as u64, h as u64);
    if w64 > MAX_DIM || h64 > MAX_DIM || w64 * h64 > MAX_PIXELS {
        (api.image_handle_release)(handle);
        return Err(DecodeError::Bad(format!(
            "图尺寸越界 {w}x{h}(JPEG 域 ≤{MAX_DIM},总量帽 {MAX_PIXELS}px)"
        )));
    }
    // 解码到 RGB 交错面(handle 活到解码终了;逐早退臂手工释放,无 RAII
    // 守卫面——所有 return 路径先 release 再返,ctx 归调用方释放)。
    let mut img: HeifImage = std::ptr::null_mut();
    let err = (api.decode_image)(
        handle,
        &mut img,
        COLORSPACE_RGB,
        CHROMA_INTERLEAVED_RGB,
        std::ptr::null(),
    );
    (api.image_handle_release)(handle);
    if err.code != HEIF_ERROR_OK {
        return Err(DecodeError::Bad(format!(
            "HEVC 解码失败: {}",
            heif_msg(&err)
        )));
    }
    let mut stride = 0usize;
    let plane = (api.image_get_plane_readonly2)(img, CHANNEL_INTERLEAVED, &mut stride);
    if plane.is_null() {
        (api.image_release)(img);
        return Err(DecodeError::Bad("取 RGB 像素面失败".into()));
    }
    // 逐行收 RGB(stride ≥ w*3,行间空隙不进 JPEG;解出的图尺寸以像素
    // 面为准——含 irot/imir 等几何变换后的呈现尺寸)。
    let mut rgb = Vec::with_capacity(
        w64.checked_mul(h64)
            .and_then(|n| n.checked_mul(3))
            .expect("已过帽") as usize,
    );
    let row_bytes = w as usize * 3;
    for y in 0..h as usize {
        let row = plane.add(y * stride);
        rgb.extend_from_slice(std::slice::from_raw_parts(row, row_bytes));
    }
    (api.image_release)(img);
    encode_jpeg(&rgb, w as u16, h as u16)
}

/// RGB 帧 → JPEG 字节(纯 Rust `jpeg-encoder`;失败 = 编码器面,映 Bad)。
fn encode_jpeg(rgb: &[u8], w: u16, h: u16) -> Result<Vec<u8>, DecodeError> {
    let mut out = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut out, JPEG_QUALITY);
    encoder
        .encode(rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| DecodeError::Bad(format!("JPEG 编码失败: {e}")))?;
    Ok(out)
}

/// `heif_error.message` → String(头文件口径恒非 NULL;防御空指针)。
unsafe fn heif_msg(err: &HeifError) -> String {
    if err.message.is_null() {
        return format!("code={}", err.code);
    }
    CStr::from_ptr(err.message).to_string_lossy().into_owned()
}

// ---------- 测试面:libheif 自编码夹具(往返自足,免外部二进制) ----------

#[cfg(test)]
pub(crate) mod fixture {
    use super::*;
    use std::path::Path;

    /// 现编一枚 HEIC 夹具(x265 HEVC 编码;48×32 棋盘渐变):写 `path`,
    /// 返回字节数。测试环境 DLL 须带 x265(`vcpkg install
    /// libheif:x64-windows` 默认含;打包 DLL 不带,见模块注)。
    pub(crate) fn encode_test_heic(w: u32, h: u32, path: &Path) -> u64 {
        let api = super::api().expect("libheif.dll 须在 PATH(heic 测试环境)");
        unsafe {
            let ctx = (api.context_alloc)();
            assert!(!ctx.is_null(), "heif_context_alloc 失败");
            let result = encode_with(api, ctx, w, h, path);
            (api.context_free)(ctx);
            result.expect("HEIC 夹具编码失败")
        }
    }

    unsafe fn encode_with(
        api: &HeifApi,
        ctx: HeifContext,
        w: u32,
        h: u32,
        path: &Path,
    ) -> Result<u64, String> {
        let mut img: HeifImage = std::ptr::null_mut();
        let err = (api.image_create)(
            w as i32,
            h as i32,
            COLORSPACE_RGB,
            CHROMA_INTERLEAVED_RGB,
            &mut img,
        );
        if err.code != HEIF_ERROR_OK {
            return Err(format!("image_create: {}", heif_msg(&err)));
        }
        let err = (api.image_add_plane)(img, CHANNEL_INTERLEAVED, w as i32, h as i32, 8);
        if err.code != HEIF_ERROR_OK {
            (api.image_release)(img);
            return Err(format!("image_add_plane: {}", heif_msg(&err)));
        }
        let mut stride = 0usize;
        let plane = (api.image_get_plane2)(img, CHANNEL_INTERLEAVED, &mut stride);
        assert!(!plane.is_null(), "image_get_plane2 返回空面");
        // 棋盘渐变:逐像素可复算,解码后可对照(测试只钉尺寸,值面不钉)。
        for y in 0..h {
            let row = plane.add(y as usize * stride);
            for x in 0..w {
                let px = row.add(x as usize * 3);
                *px = ((x * 4) % 256) as u8;
                *px.add(1) = ((y * 5) % 256) as u8;
                *px.add(2) = (((x + y) * 3) % 256) as u8;
            }
        }
        let mut encoder: HeifEncoder = std::ptr::null_mut();
        let err = (api.get_encoder_for_format)(ctx, COMPRESSION_HEVC, &mut encoder);
        if err.code != HEIF_ERROR_OK {
            (api.image_release)(img);
            return Err(format!(
                "无 HEVC 编码器(测试 DLL 须带 x265: vcpkg install libheif:x64-windows): {}",
                heif_msg(&err)
            ));
        }
        let err = (api.encoder_set_lossy_quality)(encoder, 70);
        if err.code != HEIF_ERROR_OK {
            (api.encoder_release)(encoder);
            (api.image_release)(img);
            return Err(format!("set_lossy_quality: {}", heif_msg(&err)));
        }
        let err =
            (api.context_encode_image)(ctx, img, encoder, std::ptr::null(), std::ptr::null_mut());
        (api.encoder_release)(encoder);
        (api.image_release)(img);
        if err.code != HEIF_ERROR_OK {
            return Err(format!("encode_image: {}", heif_msg(&err)));
        }
        let cpath = std::ffi::CString::new(path.to_str().expect("测试路径须 UTF-8"))
            .expect("路径无内嵌 NUL");
        let err = (api.context_write_to_file)(ctx, cpath.as_ptr());
        if err.code != HEIF_ERROR_OK {
            return Err(format!("write_to_file: {}", heif_msg(&err)));
        }
        Ok(std::fs::metadata(path)
            .map(|m| m.len())
            .expect("夹具已写盘"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::heic::fixture::encode_test_heic;

    /// 嗅探矩阵(brief「非 HEIC→404」):图片品牌五枚全过;序列/AV1 品牌
    /// 与垃圾字节、短流、错位 ftyp 全拒。
    #[test]
    fn sniff_brand_matrix() {
        let mk = |brand: &str| {
            let mut b = vec![0, 0, 0, 32];
            b.extend_from_slice(b"ftyp");
            b.extend_from_slice(brand.as_bytes());
            b.extend_from_slice(&[0u8; 24]);
            b
        };
        for brand in ["heic", "heix", "heim", "heis", "mif1"] {
            assert!(sniff_heic(&mk(brand)), "{brand} 必嗅出");
        }
        for brand in ["hevc", "hevx", "avif", "isom", "mp42"] {
            assert!(!sniff_heic(&mk(brand)), "{brand} 必拒(非静态图品牌)");
        }
        assert!(!sniff_heic(b"hello not an image at all"), "垃圾字节拒");
        assert!(!sniff_heic(b"\x89PNG\r\n\x1a\njunkjunkjunk"), "PNG 头拒");
        assert!(!sniff_heic(&[0u8; 12]), "全零拒");
        assert!(
            !sniff_heic(&[0, 0, 0, 32, b'f', b't', b'y', b'p']),
            "短流拒(<12)"
        );
        assert!(
            !sniff_heic(&[0, 0, 0, 32, 0, 0, 0, 0, b'f', b't', b'y', b'p']),
            "错位 ftyp 拒"
        );
    }

    /// 往返自足(brief 测试面):libheif 现编 64×48 HEIC → 解码转 JPEG →
    /// image crate 解码验证尺寸与格式。损坏件(截半,保 ftyp 头)→ 解码
    /// 失败 Bad(不 panic)。
    #[test]
    fn heic_roundtrip_to_jpeg_and_corrupt_fails() {
        let _guard = TEST_SERIAL.blocking_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("photo.heic");
        let len = encode_test_heic(64, 48, &path);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() as u64, len);
        assert!(sniff_heic(&bytes), "夹具必嗅出(heic 品牌)");

        let jpeg = decode_heic_to_jpeg(&bytes).expect("解码转码成功");
        let img = image::load_from_memory(&jpeg).expect("JPEG 可解");
        assert_eq!((img.width(), img.height()), (64, 48), "尺寸往返一致");

        // 损坏件:截半(ftyp 头保留,mdat 残缺)→ 解码失败,不 panic
        let cut = bytes.len() / 2;
        assert!(cut > 12, "截断长度须保住 ftyp 头");
        let got = decode_heic_to_jpeg(&bytes[..cut]);
        match got {
            Err(DecodeError::Bad(msg)) => assert!(!msg.is_empty(), "损坏面携原因: {msg}"),
            Err(other) => panic!("损坏件应映 Bad: {other:?}"),
            Ok(_) => panic!("截半件不得解码成功(长度 {len} → 截 {cut})"),
        }
    }
}
