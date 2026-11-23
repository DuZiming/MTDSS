//! 预览弹出独立窗(V4-1 T20 ⧉;field 重架构 2026-11-22)。
//!
//! ## 为什么不再运行时建窗(field 白屏根因)
//! `WebviewWindowBuilder::build()` 从命令上下文运行时建窗,在本机
//! WebView2 实测**窗口框架出现而 webview 永不渲染**(纯白、无 JS、无
//! devtools、Esc/X 失效;载 index.html 同白=与资产/脚本无关)。
//! 修法=**config 预建隐藏窗**(`tauri.conf.json windows[]` 的
//! `preview-popout`,`url: previewer.html, visible: false`)——与主窗
//! 同一条启动创建路径(已证可行);弹窗请求=改标题 + eval 注入 boot +
//! show。v1 约束:单弹窗复用(多开能力随本重架构暂时让位)。
//!
//! ## boot 注入
//! 弃 initialization_script(wry/WebView2 注册竞态,见 previewer-main
//! 注):页面侧轮询等待 `window.__MTDSS_PREVIEW_BOOT__`,本侧 eval 注入
//! (3×500ms 重试;页面轮询容忍任意时延)。诊断染色脚本同通道。
//!
//! ## 关窗语义
//! 页面 JS onCloseRequested/Esc → preventDefault + hide(不 destroy:
//! destroy 后无法复用;句柄引用随 hide 收口)。

use serde_json::Value;
use tauri::Manager as _;

/// 弹窗尺寸(config 预建窗同值;此处仅文档锚)。
const POPOUT_SIZE: (f64, f64) = (960.0, 640.0);

/// field 诊断日志(2026-11-22 二开无反应归因轮):追加写 ~/.mtdss/popout-debug.log
fn diag_log(step: &str) {
    use std::io::Write as _;
    let path = std::env::var_os("USERPROFILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_default()
        .join(".mtdss")
        .join("popout-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {step}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis());
    }
}

#[tauri::command]
pub async fn preview_popout(
    app: tauri::AppHandle,
    payload: Value,
) -> Result<(), String> {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("预览")
        .to_string();
    let boot = payload.get("boot").cloned().unwrap_or(Value::Null);
    let boot_js = format!(
        "window.__MTDSS_PREVIEW_BOOT__ = {}; window.dispatchEvent(new Event('mtdss-boot-set'));",
        serde_json::to_string(&boot).map_err(|e| format!("boot 载荷序列化失败: {e}"))?
    );
    // 诊断染色(与 previewer-main 的轮询共存;canvas 在=红/有 DOM 无画布=橙/根空=蓝)
    let diag_js = "setInterval(function(){try{var c=document.querySelectorAll('canvas').length;\
var r=(document.getElementById('root')||{innerHTML:''}).innerHTML.length;\
document.body.style.background=c>0?'crimson':(r>0?'orange':'royalblue');\
}catch(e){}},500);";

    diag_log(&format!("cmd entry: {title}"));
    // field 自愈(2026-11-22):预建窗可被 OS 路径销毁(实测第二弹 get 即
    // None)——缺窗 = 运行时重建(visible 起步,避免隐藏窗注入丢弃族问题),
    // 注入改 10×500ms 长重试(页面永续轮询 boot,eval 只需命中一次)。
    let win = match app.get_webview_window("preview-popout") {
        Some(w) => w,
        None => {
            use tauri::webview::WebviewWindowBuilder;
            use tauri::WebviewUrl;
            diag_log("window missing — runtime recreate");
            WebviewWindowBuilder::new(
                &app,
                "preview-popout",
                WebviewUrl::App("previewer.html".into()),
            )
            .title(&title)
            .inner_size(POPOUT_SIZE.0, POPOUT_SIZE.1)
            .visible(true)
            .build()
            .map_err(|e| format!("重建预览窗失败: {e}"))?
        }
    };
    win.set_title(&title)
        .map_err(|e| format!("设置标题失败: {e}"))?;
    diag_log("set_title done");
    // field 顺序修(2026-11-22 关后再打不开):先 show 再注入——隐藏窗
    // 的 eval/事件派发可被 WebView2 丢弃(首次弹窗 = 启动隐藏期间 eval
    // 曾全灭的同族面);可见后 300ms 注入,三轮重试兜底。
    win.show().map_err(|e| format!("显示预览窗失败: {e}"))?;
    diag_log("show done");
    // field 终形:离屏停放复活 = 居中回屏(不依赖 hide/show 状态机)
    let _ = win.center();
    diag_log("center done");
    let _ = win.set_focus();
    {
        let win_eval = win.clone();
        let boot_js = boot_js.clone();
        let _ = diag_js.to_string(); // field 诊断染色脚本(未 eval;保留锚,归因轮后撤)
        tauri::async_runtime::spawn(async move {
            for i in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if win_eval.eval(&boot_js).is_ok() {
                    diag_log(&format!("eval ok @{i}"));
                    break;
                }
                diag_log(&format!("eval retry {i}"));
            }
            // field 诊断 r10f(2026-11-23,弹窗 chrome-error 归因轮):探针先
            // 抄走当前页 innerText(错误页含 ERR_ 码)再覆盖渲染;主窗同探
            // 对照。定位后整段撤。
            if std::env::var("MTDSS_POPUP_TEST").ok().as_deref() == Some("1") {
                tokio::time::sleep(std::time::Duration::from_secs(8)).await;
                let probe = concat!(
                    "setInterval(function(){try{",
                    "var o=[];",
                    "var et=(document.body&&document.body.innerText||'').replace(/\\s+/g,' ');",
                    "if(et.indexOf('DIAG')===0||et.indexOf('DIAGERR')===0)et='(selfecho)';",
                    "et=et.slice(0,160);",
                    "var o=[];",
                    "var m=document.querySelector('meta[http-equiv=\"Content-Security-Policy\" i]');",
                    "var c=m?m.content:'NO-CSP-META';",
                    "var ss=(c.match(/style-src[^;]*/)||['NO-style-src'])[0];",
                    "var cs=(c.match(/script-src[^;]*/)||['NO-script-src'])[0];",
                    "document.body.innerHTML='<pre id=mtdss-diag style=\"position:fixed;left:0;top:0;z-index:99999;margin:0;font:19px/1.6 monospace;background:#fff;color:#000;white-space:pre-wrap;word-break:break-all\">STYLE1 '+ss+'\\nSCRIPT1 '+cs+'</pre>';return;",
                    "var t=o.join(' ~ ');",
                    "document.body.innerHTML='<pre id=mtdss-diag style=\"position:fixed;left:0;top:0;z-index:99999;margin:0;font:15px/1.5 monospace;background:#fff;color:#000;white-space:pre-wrap;word-break:break-all\">DIAG '+t+'</pre>';",
                    "}catch(e){try{document.body.innerHTML='<pre style=\"position:fixed;left:0;top:0;font:16px monospace\">DIAGERR '+e+'</pre>';}catch(_){}}},2000);"
                );
                diag_log("probe: firing");
                match win_eval.eval(probe) {
                    Ok(()) => diag_log("probe: eval ok"),
                    Err(e) => diag_log(&format!("probe: eval err {e}")),
                }
                // 主窗同探对照(同 spawn;主窗永可见,失败面归因对照)
                if let Some(main) = win_eval.app_handle().get_webview_window("main") {
                    let probe2 = probe.to_string();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        let _ = main.eval(&probe2);
                    });
                }
            }
        });
    }
    let _ = POPOUT_SIZE; // 文档锚(实际尺寸在 config)
    Ok(())
}
