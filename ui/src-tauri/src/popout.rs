//! 预览弹出独立窗(V4-1 T20 ⧉;field r12 多开重构 2026-11-23:用户裁定
//! 「再点=再弹一窗,不复用不置换」——标签池见 POPOUT_LABELS)。
//!
//! ## 建窗路径
//! 首发弹窗用 config 预建窗(tauri.conf `preview-popout`,隐藏起步)的
//! show 面;后续每弹窗运行时创建(可见起步,自愈同款已证路径)。
//!
//! ## boot 注入
//! 弃 initialization_script(wry/WebView2 注册竞态,见 previewer-main
//! 注):页面侧轮询等待 `window.__MTDSS_PREVIEW_BOOT__`,本侧 eval 注入
//! (10×500ms 重试;页面轮询容忍任意时延)。诊断探针同通道(测试钩子)。
//!
//! ## 关窗语义(r12)
//! 页面 JS Esc/X → 真销毁(不再 preventDefault/离屏停放);句柄经页内
//! CloseRequested 收口。窗销毁后同标签可再创建(自愈)。

use serde_json::Value;
use tauri::Manager as _;

/// 弹窗尺寸(config 预建窗同值;此处仅文档锚)。
const POPOUT_SIZE: (f64, f64) = (960.0, 640.0);

/// 弹窗标签池(field r12 多开重构 2026-11-23,用户裁定「再点=再弹一窗,
/// 不复用不置换」):每弹窗独立窗。config 预建窗占首位,其余运行时创建
/// (自愈同款可见起步路径,已证可渲染);上限 8 窗,全忙时关编号最小者
/// (=最早在用)复用其标签,资源有界。
const POPOUT_LABELS: [&str; 8] = [
    "preview-popout",
    "preview-popout-2",
    "preview-popout-3",
    "preview-popout-4",
    "preview-popout-5",
    "preview-popout-6",
    "preview-popout-7",
    "preview-popout-8",
];

/// 取一个空闲标签(r12 多开):优先沉睡的 config 预建窗(存在且不可见=
/// 从未弹过,首发弹窗走已验证的 show 路径);否则第一个不存在的编号标签
/// (运行时创建);全忙则销毁编号最小的在用窗(最旧)复用其标签。
/// 不可见判定失败(窗口管理器竞态)按在用处理,宁多开不抢占。
fn pick_popout_label(app: &tauri::AppHandle) -> String {
    use tauri::Manager as _;
    if let Some(w) = app.get_webview_window(POPOUT_LABELS[0]) {
        match w.is_visible() {
            Ok(false) => return POPOUT_LABELS[0].to_string(), // 沉睡预建窗
            _ => {}                                           // 在用/判态失败 → 走编号池
        }
    }
    for label in POPOUT_LABELS.iter().skip(1) {
        if app.get_webview_window(label).is_none() {
            return label.to_string();
        }
    }
    let oldest = POPOUT_LABELS
        .iter()
        .find(|l| app.get_webview_window(l).is_some())
        .copied()
        .expect("全忙分支必有在用窗");
    if let Some(w) = app.get_webview_window(oldest) {
        let _ = w.destroy();
    }
    oldest.to_string()
}

/// 弹窗页 URL 防缓存 nonce(field r16 #2:WebView2 对 tauri.localhost 的
/// 入口 html(previewer.html,URL 恒定)走启发式缓存——升级安装不清用户
/// 数据,旧版 html+旧 hash JS 可整体命中陈缓存,弹窗跑旧代码(三件套
/// 「没做出来」的现场形态)。每次进程运行一枚 nonce,config 预建窗首发
/// 强制导航带参 URL,运行时建窗直接带参——入口 URL 变即缓存必 miss;
/// hash 资产名自版本,二级缓存无害。
fn popout_page_url() -> String {
    use std::sync::OnceLock;
    static NONCE: OnceLock<String> = OnceLock::new();
    let n = NONCE.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
            .to_string()
    });
    format!("previewer.html?v={n}")
}

/// config 预建窗本运行是否已带参导航(一次性;导航后页面重载,boot 由
/// eval 重试环注入——页侧永续轮询容忍任意时延)。
static CONFIG_WINDOW_NONCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    // field r12 多开:每弹窗取独立标签(空闲优先,全忙逐最旧);命中在
    // 用窗(理论不可达——标签只发给无窗者)走复用面,缺窗=运行时创建
    // (可见起步,自愈同款;页面永续轮询 boot,eval 只需命中一次)。
    // r16 防缓存:运行时建窗直接带 nonce URL。
    let label = pick_popout_label(&app);
    diag_log(&format!("label: {label}"));
    let page = popout_page_url();
    let win = match app.get_webview_window(&label) {
        Some(w) => {
            // config 预建窗(隐藏起步):首发强制导航带参 URL 一次——旧版
            // html 缓存防线(见 popout_page_url 注);导航触发页面重载,
            // boot 注入由下方重试环兜住。
            if label == POPOUT_LABELS[0]
                && !CONFIG_WINDOW_NONCED.swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                if let Ok(url) = tauri::Url::parse(&format!("http://tauri.localhost/{page}")) {
                    diag_log("config window: nonce navigate");
                    let _ = w.navigate(url);
                }
            }
            w
        }
        None => {
            use tauri::webview::WebviewWindowBuilder;
            use tauri::WebviewUrl;
            diag_log("window missing — runtime create");
            WebviewWindowBuilder::new(&app, label.clone(), WebviewUrl::App(page.into()))
                .title(&title)
                .inner_size(POPOUT_SIZE.0, POPOUT_SIZE.1)
                .visible(true)
                .build()
                .map_err(|e| format!("创建预览窗失败: {e}"))?
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
                    "(function(){try{",
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
                    "}catch(e){try{document.body.innerHTML='<pre style=\"position:fixed;left:0;top:0;font:16px monospace\">DIAGERR '+e+'</pre>';}catch(_){}}})();"
                );
                diag_log("probe: firing");
                match win_eval.eval(probe) {
                    Ok(()) => diag_log("probe: eval ok"),
                    Err(e) => diag_log(&format!("probe: eval err {e}")),
                }
                // (r12 关窗链诊断已撤:preventDefault+destroy 定形后,页内
                // 直调 close 会真销毁窗口,诊断自身变成了副作用面。)
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
