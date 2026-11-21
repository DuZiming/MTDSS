//! Windows 缩略图后端(V4-1 T7):Shell `IShellItemImageFactory`(COM)。
//!
//! ## 选型(spec §4 钉死:IShellItemImageFactory,系统原生,零第三方解码)
//!
//! `SHCreateItemFromParsingName(path, IID_IShellItemImageFactory)` →
//! `GetImage(SIZE{w,w}, SIIGBF_THUMBNAILONLY)` → HBITMAP → `GetDIBits`
//! 取 32bpp BGRA(顶朝下)→ 去 alpha 转 RGB → 纯 Rust `jpeg-encoder`
//! 编码(质量 90,heic 转码面同值)。系统原生出图,**覆盖图片/视频抽帧
//! /PDF 首页/HEIC**——解码器由 Windows Shell 缩略图管线(WIC/媒体源/
//! 第三方注册的缩略图处理程序)承担,本 crate 零 C 依赖。
//!
//! `SIIGBF_THUMBNAILONLY`(≠ 默认 0)是契约关键:Shell 提不出**真缩略
//! 图**的文件类型(.txt/.xyz 等无后端类型)如实失败——引擎映空字节,客
//! 户端回退类型图标;绝不让 Shell 拿一个通用图标位图充数(brief「无后
//! 端文件类型→空字节」的回归红线)。`SIIGBF_RESIZETOFIT`(=0,可与其他
//! 旗标或合)保证等比缩放入槽、不放大。
//!
//! COM 卫生:线程级 `CoInitializeEx(COINIT_MULTITHREADED)`——后台阻塞池
//! 线程无消息泵,MTA 免跨套间 marshaling 悬挂;`RPC_E_CHANGED_MODE`(线
//! 程已被他处初始化为他套间)按「非我初始化」处理,照样尝试,收尾免
//! `CoUninitialize`。每次调用 init/uninit 平衡收口(阻塞池线程复用,不
//! 泄会话);对象/GDI 句柄逐早退臂手工释放(heic.rs 同款纪律,无 RAII
//! 守卫面)。
//!
//! 依赖:`windows` crate(类型化 COM 绑定;经 notify-rust 链已在 Windows
//! 构建树内,特性增配零新增下载)。调用点全在 spawn_blocking(调用方
//! [`super::thumb`] 责任);本函数纯同步。

#![cfg(windows)]

use std::path::Path;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, BITMAPINFO, BITMAPINFOHEADER,
    DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::{
    IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_THUMBNAILONLY,
};

/// JPEG 编码质量(预览面:heic 转码面同值 90,足够锐不追极致)。
const JPEG_QUALITY: u8 = 90;

/// RPC_E_CHANGED_MODE(线程已被他处初始化为他套间)。
const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

/// 位图边长安全帽(GetImage 以请求宽为上界、且不放大,常态 ≤256;此处
/// 只防「源端缩略图处理程序越界出巨图」的异常面,2560 = Shell jumbo 档
/// 上界量级;超帽按失败拒,不进编码面)。
const MAX_BITMAP_EDGE: i32 = 2560;

/// Shell 缩略图提取:路径 + 目标宽 → JPEG 字节;失败/提不出缩略图 =
/// None(引擎映空字节)。阻塞调用(spawn_blocking 由调用方收口)。
pub(crate) fn shell_thumbnail(path: &Path, w: u32) -> Option<Vec<u8>> {
    // COM 会话(线程级):MTA——后台阻塞池线程无消息泵,免跨套间
    // marshaling 悬挂。S_OK/S_FALSE(已初始化)都要 uninit 平衡;
    // RPC_E_CHANGED_MODE(线程已是 STA)= 非我形态,快失败不尝试
    // (STA 上调 GetImage 可能等消息泵,悬等比缺图更糟);其余失败 =
    // COM 起不来,无缩略图。
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let ours = match hr.0 {
        0 | 1 => true, // S_OK / S_FALSE
        RPC_E_CHANGED_MODE => {
            tracing::debug!("COM 线程套间不符(STA 在场),缩略图快失败");
            false
        }
        _ => {
            tracing::debug!(hr = hr.0, "CoInitializeEx 失败,缩略图不可用");
            false
        }
    };
    let out = if ours {
        shell_thumbnail_inner(path, w)
    } else {
        None
    };
    if ours {
        unsafe { CoUninitialize() };
    }
    out
}

/// 生成主体(COM 会话在场;COM 对象/GDI 句柄逐早退臂手工释放)。
fn shell_thumbnail_inner(path: &Path, w: u32) -> Option<Vec<u8>> {
    // 宽钳制(引擎面已 clamp,此处冗余防线;0/巨宽都不进 COM 面)
    let w = w.clamp(1, super::thumb::THUMB_MAX_WIDTH) as i32;

    // 路径宽字符(PCWSTR;NUL 结尾)
    use std::os::windows::ffi::OsStrExt as _;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);

    unsafe {
        // 工厂:Shell 按解析名出 IShellItemImageFactory(走缩略图缓存/
        // 在线生成管线)
        let factory: IShellItemImageFactory =
            SHCreateItemFromParsingName(windows::core::PCWSTR::from_raw(wide.as_ptr()), None)
                .ok()?;

        // 提图:THUMBNAILONLY = 提不出真缩略图如实失败(见模块注)
        let hbm: HBITMAP = match factory.GetImage(SIZE { cx: w, cy: w }, SIIGBF_THUMBNAILONLY) {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!(path = %path.display(), err = %e, "Shell 缩略图提取失败");
                return None;
            }
        };
        // HBITMAP 所有权自此归本函数:所有早退臂先 DeleteObject
        let out = hbitmap_to_jpeg(hbm, path);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        out
    }
}

/// HBITMAP → JPEG(GetDIBits 顶朝下 32bpp → 去 alpha → jpeg-encoder)。
/// hdc 兼容 DC 取自过程堆,终了 DeleteDC(逐早退臂手工收口)。
unsafe fn hbitmap_to_jpeg(hbm: HBITMAP, path: &Path) -> Option<Vec<u8>> {
    let hdc: HDC = CreateCompatibleDC(None);
    if hdc.is_invalid() {
        tracing::debug!(path = %path.display(), "CreateCompatibleDC 失败");
        return None;
    }
    // 第一发:lpvBits=None,让 GDI 按实图填 bmiHeader(宽/高/位深)
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    if GetDIBits(hdc, hbm, 0, 0, None, &mut bmi, DIB_RGB_COLORS) == 0 {
        tracing::debug!(path = %path.display(), "GetDIBits 探头失败");
        let _ = DeleteDC(hdc);
        return None;
    }
    let (w, h) = (bmi.bmiHeader.biWidth, bmi.bmiHeader.biHeight.abs());
    if w <= 0 || h <= 0 || w > MAX_BITMAP_EDGE || h > MAX_BITMAP_EDGE {
        tracing::debug!(path = %path.display(), w, h, "位图尺寸越界帽");
        let _ = DeleteDC(hdc);
        return None;
    }
    // 第二发:按探得的尺寸收 BGRA(顶朝下:biHeight 负值)
    let row_bytes = (w as usize) * 4;
    let mut buf = vec![0u8; row_bytes * (h as usize)];
    bmi.bmiHeader.biHeight = -h; // 顶朝下(UI 面零翻转)
    bmi.bmiHeader.biBitCount = 32;
    bmi.bmiHeader.biCompression = 0; // BI_RGB
    let n = GetDIBits(
        hdc,
        hbm,
        0,
        h as u32,
        Some(buf.as_mut_ptr() as *mut std::ffi::c_void),
        &mut bmi,
        DIB_RGB_COLORS,
    );
    let _ = DeleteDC(hdc);
    if n == 0 {
        tracing::debug!(path = %path.display(), "GetDIBits 取面失败");
        return None;
    }
    // BGRA → RGB(丢 alpha;缩略图面无 alpha 语义,heic 转码面同口径)
    let mut rgb = Vec::with_capacity((w as usize) * (h as usize) * 3);
    for px in buf.as_chunks::<4>().0 {
        rgb.push(px[2]); // B
        rgb.push(px[1]); // G
        rgb.push(px[0]); // R
    }
    let mut jpeg = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut jpeg, JPEG_QUALITY);
    encoder
        .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .ok()?;
    Some(jpeg)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 边界锚:JPEG_QUALITY 与 heic 转码面同值(预览面口径一致)。
    #[test]
    fn quality_anchored_to_heic_face() {
        assert_eq!(JPEG_QUALITY, 90);
    }
}
