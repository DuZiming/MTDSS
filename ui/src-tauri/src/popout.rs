//! 独立预览窗(V4-1 T20 ⧉):Tauri WebWindow 载 previewer 页
//! (previewer.html,vite 多页入口),boot 载荷(PreviewRequest+句柄)经
//! initialization_script 注入——webview 级用户脚本,CSP 不约束、无 query
//! 编码面。多开允许:label 递增计数(preview-popout-{n});窗口标题=文件
//! 名。窗内 previewer-main.ts 自持 Esc=关窗与 onCloseRequested=previewClose
//! 收口本窗句柄引用(daemon preview_close 桥命令);UI 进程崩溃漏关的残
//! 项由 daemon 空闲回收兜底(T4 既有语义)。多屏 DPI 打磨归冒烟(brief
//! 不做面)。

use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::Value;
use tauri::{AppHandle, WebviewUrl, WebviewWindowBuilder};

/// 弹出窗 label 递增计数(多开允许;label 进程内唯一即可,多屏多窗互不
/// 干涉——每窗独立 webview 各自 mountPreview)。
static POPOUT_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 独立预览窗缺省尺寸(960×640,存档窗族量级;用户可调 OS 缘)。
const POPOUT_SIZE: (f64, f64) = (960.0, 640.0);

/// `preview_popout {payload:{title,boot:{req,handle}}}`(V4-1 T20 桌面桥
/// 新增本机命令,非 RPC——纯建窗,无返回载荷):title=窗口标题(文件名
/// 钉面),boot 经初始化脚本注入窗内全局 `window.__MTDSS_PREVIEW_BOOT__`
/// (previewer-main.ts 消费:mountPreview+关窗收口)。载荷解析宽松——
/// title 缺省「预览」,boot 缺席注入 null(窗内自示「缺少预览参数」防御
/// 面,不 panic);建窗失败上抛(App 侧 toast)。
#[tauri::command]
pub fn preview_popout(app: AppHandle, payload: Value) -> Result<(), String> {
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("预览")
        .to_string();
    let boot = payload.get("boot").cloned().unwrap_or(Value::Null);
    let script = format!(
        "window.__MTDSS_PREVIEW_BOOT__ = {};",
        serde_json::to_string(&boot).map_err(|e| format!("boot 载荷序列化失败: {e}"))?
    );
    let label = format!("preview-popout-{}", POPOUT_SEQ.fetch_add(1, Ordering::Relaxed));
    let win = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("previewer.html".into()))
        .title(&title)
        .inner_size(POPOUT_SIZE.0, POPOUT_SIZE.1)
        .initialization_script(&script)
        .build()
        .map_err(|e| format!("打开预览窗口失败: {e}"))?;
    let _ = win.set_focus();
    Ok(())
}
