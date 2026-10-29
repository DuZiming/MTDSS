//! dropresolve:拖出落点解析(Task 10 spike)。
//!
//! # 解决什么
//! 用户把远端文件从 MTDSS 窗口拖出、在 Windows 桌面或某资源管理器窗口上松开鼠标时,
//! 判定鼠标下方是「桌面(稳定路径)」还是「某个资源管理器窗口的当前文件夹」,供
//! Task 12 直接把下载落进该目录;其余目标(库视图 / 此电脑 / 回收站 / `::{GUID}` /
//! UNC / 任何非桌面非 CabinetWClass 窗口)一律 [`DropTarget::Unresolvable`]。
//!
//! # 解析管线(Windows)
//! `GetCursorPos` → `WindowFromPoint` → `GetAncestor(GA_ROOT)`(桌面图标列表是
//! Progman 的孙窗口,不取根会把类名看成 SysListView32)→ `GetClassNameW`:
//! - `Progman` / `WorkerW` → 桌面:`SHGetKnownFolderPath(FOLDERID_Desktop)` 取稳定
//!   路径(不信任「当前目录」,桌面路径随用户配置/OneDrive 重定向)。
//! - `CabinetWClass` → 资源管理器:UIA 走地址栏(见下)。
//! - 其余 → Unresolvable(任务栏 Shell_TrayWnd、浏览器、MTDSS 自身窗口等都落这)。
//!
//! # UIA 走法(spike 在 Win11 Enterprise 22000 上探明的结论)
//! windows-sys **不含任何 COM 接口 vtable**(只有 C 风格 Uia* API,而后者遍历
//! 树必须经 IUiaCacheRequest COM 方法,同样要 vtable)——因此本模块内手写三个
//! 最小 vtable(`#[repr(C)]`,槽位逐个对照本机 SDK UIAutomationClient.h 校验:
//! IUIAutomation 5 GetRootElement/6 ElementFromHandle/23 CreatePropertyCondition;
//! IUIAutomationElement 5 FindFirst/6 FindAll/10 GetCurrentPropertyValue;
//! IUIAutomationElementArray 3 GetLength/4 GetElement)。
//!
//! 条件对象(IUIAutomationCondition)与元素指针全部当不透明 `*mut c_void` 用,
//! 引用计数经统一 `release`(vtable 槽 2)释放;不 QueryInterface,不碰更深的
//! 元素 getter 槽位(属性统一走 GetCurrentPropertyValue + VARIANT,读法单一)。
//!
//! 地址栏取值(本机实测的关键事实:地址栏是 `ToolbarWindow32`(AutomationId
//! **1001**,Win7→11 带状结构沿用),UIA 控件类型是 Pane 而非 ToolBar,其
//! **Name 就是「地址: C:\\完整原始路径」**;非聚焦态下不存在持有路径的 Edit,
//! 「隐藏 Edit 持有原始路径」的经典说法在本构建不成立):
//! 1. 主路:`ElementFromHandle`(窗口根,约 30ms)→ `FindFirst(AutomationId=
//!    1001)`(树序短路,顶层 ReBar 带先于文件列表命中,约 35ms)→ 校验类名
//!    ToolbarWindow32 → Name 剥「地址:」标签 → `parse_address_bar`;
//! 2. 聚焦态补充:该工具栏子树内的 Edit 的 Value(用户点进地址栏时才存在);
//! 3. 兜底(仅 aid 1001 缺席的异构建型):窗口内 `ToolbarWindow32` 名称全扫
//!    (实测约 0.2s);
//! 4. 候选再退一步认驱动器根显示名「本地磁盘 (C:)」形态(见 drive_display_root);
//! 5. 已知文件夹显示名兜底(r2c 冒烟补):侧栏/快速访问打开的已知文件夹
//!    (下载/桌面/文档等),其地址栏 Name 只有本地化显示名(`地址: 下载`,
//!    无盘符路径),1–4 全灭;此时按显示名精确映射(zh + en 两套常用名,见
//!    known_folder_display_path)→ SHGetKnownFolderPath → 存在性校验 → 同
//!    ExplorerDir。仅覆盖中英文 locale,其他语言仍 Unresolvable(后续路子 =
//!    ShellWindows LocationURL,另立票)。
//!
//! 候选全灭 → Unresolvable,原因里带上原始候选文本,便于冒烟手册核对。
//!
//! COM 按线程初始化,用 **MTA**:解析跑在 spawn_blocking 线程池线程上,STA
//! 线程没有消息泵,不是跨套 COM 调用的合法居所;MTA 下实测整条 UIA 路径
//! 约 70ms。线程已被初始化成别的套(RPC_E_CHANGED_MODE)则沿用现套,不
//! uninit。任何一步 HRESULT 失败都返回 Unresolvable——**绝不 panic**,失败
//! 即降级,这就是 spike 说的「UIA 探测失败自然降级」的实现面。
//!
//! unsafe FFI 全部收口在本文件 `cfg(windows)` 的 `imp` 模块内;对外只有
//! [`resolve_drop_at_cursor`] / [`parse_address_bar`] / [`drop_resolve_at_cursor`]
//! 三个安全入口。非 Windows 平台整套为空实现(恒 Unresolvable),保编译。

use serde_json::{json, Value};
use std::path::PathBuf;

/// 拖出落点解析结果。
#[derive(Debug, Clone, PartialEq)]
pub enum DropTarget {
    /// 桌面(SHGetKnownFolderPath 的稳定 Desktop 路径,已验证存在)。
    Desktop(PathBuf),
    /// 资源管理器窗口当前文件夹(UIA 地址栏解析出且真实存在的目录)。
    ExplorerDir(PathBuf),
    /// 不可解析(非文件系统视图 / UIA 失败 / 路径不存在等;携带人可读原因)。
    Unresolvable(String),
}

/// 地址栏文本 → 文件系统路径(纯函数,单测面)。
///
/// 只认「盘符 + 冒号 + 分隔符」开头的绝对路径(大小写盘符均可、正反斜杠均可、
/// 支持中文与空格);首尾空白剥除;正斜杠统一规整为反斜杠;尾缀分隔符剥除但根
/// (`C:\`)保留。其余一切——`::{GUID}`、`shell:` 前缀、UNC、显示名(此电脑/回收站)、
/// 面包屑形态、相对路径、盘符相对(`C:foo`)、空串——均 `None`。路径是否真实
/// 存在由调用方 `std::fs` 复核,本函数只管形态。
pub fn parse_address_bar(text: &str) -> Option<PathBuf> {
    let t = text.trim();
    let bytes = t.as_bytes();
    // 最短合法形态 "C:\" / "C:/" 恰 3 字节。
    if bytes.len() < 3 {
        return None;
    }
    // 盘符必须是 ASCII 字母(此电脑/回收站/::{GUID}/UNC/相对路径/中文显示名全在此被拒)。
    if !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        return None;
    }
    let sep = bytes[2];
    if sep != b'\\' && sep != b'/' {
        return None; // "C:foo" 盘符相对路径不是绝对路径
    }
    // 分隔符规整:统一反斜杠(仅当确有正斜杠才分配新串)。
    let mut s = if t.contains('/') {
        t.replace('/', "\\")
    } else {
        t.to_string()
    };
    // 尾缀分隔符规整:剥掉所有尾随反斜杠,但保留根的("C:\" 三字节整)。
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    Some(PathBuf::from(s))
}

/// 地址栏工具栏元素 Name 的标签剥除:「地址: C:\\x」/「Address: C:\\x」→「C:\\x」
/// (冒号认半角 U+003A 与全角 U+FF1A 两种,英文标签大小写各一)。无标签 → None
/// (调用方原样使用候选文本)。
fn strip_address_label(name: &str) -> Option<&str> {
    // 第二项是全角冒号变体,用转义写法让两处差异可见、防再次隐形重复。
    for prefix in ["地址:", "地址\u{FF1A}", "Address:", "address:"] {
        if let Some(rest) = name.strip_prefix(prefix) {
            return Some(rest.trim());
        }
    }
    None
}

/// 驱动器根的「显示名形态」:「本地磁盘 (C:)」「OS (D:)」→ `C:\`。仅在
/// parse_address_bar 拒绝整个候选后才尝试(中途段不会出现该形态——普通文件夹
/// 的工具栏 Name 就是完整原始路径,先被 parse 吃掉),且结果照旧过存在性校验。
fn drive_display_root(text: &str) -> Option<PathBuf> {
    let t = text.trim();
    if !t.ends_with(')') || t.len() < 4 {
        return None;
    }
    let open = t.rfind('(')?;
    if open == 0 {
        return None; // 空标签的「(C:)」不算显示名形态
    }
    let letter = &t[open + 1..t.len() - 1];
    let b = letter.as_bytes();
    if b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        Some(PathBuf::from(format!("{letter}\\")))
    } else {
        None
    }
}

/// 已知文件夹判别值(纯面判别:映射与单测不依赖 FFI,GUID 选取收在
/// cfg(windows) 的 imp::known_folder_path)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnownFolder {
    Downloads,
    Desktop,
    Documents,
    Pictures,
    Music,
    Videos,
}

/// 已知文件夹显示名 → 判别值(纯函数,单测面;r2c)。
///
/// 侧栏/快速访问打开的已知文件夹,地址栏 UIA Name 只有本地化显示名(如
/// 「地址: 下载」,无盘符路径),parse_address_bar 与 drive_display_root 均
/// 拒之。本函数对剥「地址:」标签后的候选文本做**精确匹配**(zh + en 两套
/// 常用名,英文区分大小写),命中返回判别值;其余——近似名(下载2)、小写
/// 英文、全路径、其他命名空间名、空串——一律 None。局限:仅中英文显示名,
/// 其他语言环境仍 Unresolvable(后续路子 = ShellWindows LocationURL,另立票)。
fn known_folder_display_path(text: &str) -> Option<KnownFolder> {
    let t = text.trim();
    const TABLE: &[(&str, KnownFolder)] = &[
        ("下载", KnownFolder::Downloads),
        ("Downloads", KnownFolder::Downloads),
        ("桌面", KnownFolder::Desktop),
        ("Desktop", KnownFolder::Desktop),
        ("文档", KnownFolder::Documents),
        ("Documents", KnownFolder::Documents),
        ("图片", KnownFolder::Pictures),
        ("Pictures", KnownFolder::Pictures),
        ("音乐", KnownFolder::Music),
        ("Music", KnownFolder::Music),
        ("视频", KnownFolder::Videos),
        ("Videos", KnownFolder::Videos),
    ];
    TABLE.iter().find(|(name, _)| *name == t).map(|(_, kf)| *kf)
}

/// [`DropTarget`] → 前端 JSON 契约(Task 12 依赖):
/// `{"kind":"desktop"|"dir"|"unresolvable","path":String?,"reason":String?}`。
fn drop_target_to_json(t: &DropTarget) -> Value {
    match t {
        DropTarget::Desktop(p) => json!({
            "kind": "desktop",
            "path": p.to_string_lossy(),
            "reason": null,
        }),
        DropTarget::ExplorerDir(p) => json!({
            "kind": "dir",
            "path": p.to_string_lossy(),
            "reason": null,
        }),
        DropTarget::Unresolvable(reason) => json!({
            "kind": "unresolvable",
            "path": null,
            "reason": reason,
        }),
    }
}

/// 鼠标落点 → 拖出目标(Windows 实现;全部 FFI 在下方 `imp`)。
#[cfg(windows)]
pub fn resolve_drop_at_cursor() -> DropTarget {
    imp::resolve()
}

/// 非 Windows 平台:恒 Unresolvable(空实现,保编译)。
#[cfg(not(windows))]
pub fn resolve_drop_at_cursor() -> DropTarget {
    DropTarget::Unresolvable("仅 Windows 支持拖出落点解析".to_string())
}

/// Tauri 命令:解析当前鼠标落点。UIA 单次解析含跨进程 COM 调用(几十~几百毫秒),
/// 放 `spawn_blocking` 免得占 tokio worker;COM 按该 blocking 线程初始化(imp 内)。
#[tauri::command]
pub async fn drop_resolve_at_cursor() -> Result<Value, String> {
    tokio::task::spawn_blocking(resolve_drop_at_cursor)
        .await
        .map_err(|e| format!("落点解析任务失败:{e}"))
        .map(|t| drop_target_to_json(&t))
}

// ---------------------------------------------------------------------------
// Windows 实现:unsafe FFI 全部收口在此模块。
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use super::{
        drive_display_root, known_folder_display_path, parse_address_bar, strip_address_label,
        DropTarget, KnownFolder,
    };
    use std::ffi::c_void;
    use std::path::PathBuf;
    use windows_sys::core::{GUID, HRESULT};
    use windows_sys::Win32::Foundation::{HWND, POINT, RPC_E_CHANGED_MODE, SysAllocStringLen};
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_MULTITHREADED,
    };
    use windows_sys::Win32::System::Variant::{
        VariantClear, VariantInit, VARIANT, VT_BSTR, VT_I4,
    };
    use windows_sys::Win32::UI::Accessibility::{
        TreeScope_Descendants, UIA_AutomationIdPropertyId, UIA_ClassNamePropertyId,
        UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_NamePropertyId,
        UIA_ValueValuePropertyId,
    };
    use windows_sys::Win32::UI::Shell::{
        FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music,
        FOLDERID_Pictures, FOLDERID_Videos, SHGetKnownFolderPath,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetClassNameW, GetCursorPos, WindowFromPoint, GA_ROOT,
    };

    // ---- 手写最小 UIA vtable(槽位号照 UIAutomationClient.h;见模块文档) ----

    /// 所有 COM 接口共同的 IUnknown 头(槽 2 = Release,用于统一释放)。
    #[repr(C)]
    struct UiaUnknownHead {
        _query_interface: unsafe extern "system" fn(),
        _add_ref: unsafe extern "system" fn(),
        release: unsafe extern "system" fn(*mut c_void) -> u32,
    }

    /// IUIAutomation vtable(槽位照本机 SDK UIAutomationClient.h 校死:3
    /// CompareElements、4 CompareRuntimeIds、5 GetRootElement、6
    /// ElementFromHandle、7 ElementFromPoint、8 GetFocusedElement、…、23
    /// CreatePropertyCondition;占位槽签名无意义,永不调用)。
    #[repr(C)]
    struct UiaAutomationVTable {
        _iunknown: [unsafe extern "system" fn(); 3],       // 0-2 IUnknown
        _compare_elements: unsafe extern "system" fn(),    // 3
        _compare_runtime_ids: unsafe extern "system" fn(), // 4
        _get_root_element: unsafe extern "system" fn(),    // 5
        /// 槽 6:ElementFromHandle(UIA_HWND 即 HWND,void* 传值)。
        element_from_handle:
            unsafe extern "system" fn(this: *mut c_void, hwnd: HWND, out: *mut *mut c_void) -> HRESULT,
        _element_from_point: unsafe extern "system" fn(),  // 7
        _get_focused_element: unsafe extern "system" fn(), // 8
        _pad_9_22: [unsafe extern "system" fn(); 14],      // 9-22
        /// 槽 23:CreatePropertyCondition(条件 VARIANT 按值传)。
        create_property_condition: unsafe extern "system" fn(
            this: *mut c_void,
            property_id: i32,
            value: VARIANT,
            out: *mut *mut c_void,
        ) -> HRESULT,
    }

    /// IUIAutomationElement vtable(只需槽 6 与槽 10)。
    #[repr(C)]
    struct UiaElementVTable {
        _iunknown: [unsafe extern "system" fn(); 3],      // 0-2
        _set_focus: unsafe extern "system" fn(),         // 3
        _get_runtime_id: unsafe extern "system" fn(),    // 4
        /// 槽 5:FindFirst(scope, 条件, **元素)——树序短路,匹配即返回。
        find_first: unsafe extern "system" fn(
            this: *mut c_void,
            scope: i32,
            condition: *mut c_void,
            out: *mut *mut c_void,
        ) -> HRESULT,
        /// 槽 6:FindAll(scope, 条件, **元素数组)。条件是不透明指针。
        find_all: unsafe extern "system" fn(
            this: *mut c_void,
            scope: i32,
            condition: *mut c_void,
            out: *mut *mut c_void,
        ) -> HRESULT,
        _pad_7_9: [unsafe extern "system" fn(); 3],      // 7-9
        /// 槽 10:GetCurrentPropertyValue(一切属性读取的单一入口,BSTR 出 VARIANT)。
        get_current_property_value:
            unsafe extern "system" fn(this: *mut c_void, property_id: i32, out: *mut VARIANT) -> HRESULT,
    }

    /// IUIAutomationElementArray vtable(槽 3/4)。
    #[repr(C)]
    struct UiaElementArrayVTable {
        _iunknown: [unsafe extern "system" fn(); 3],  // 0-2
        get_length: unsafe extern "system" fn(this: *mut c_void, out: *mut i32) -> HRESULT, // 3
        get_element:
            unsafe extern "system" fn(this: *mut c_void, index: i32, out: *mut *mut c_void) -> HRESULT, // 4
    }

    /// 统一 Release(vtable 槽 2);null 安全。
    unsafe fn uia_release(p: *mut c_void) {
        unsafe {
            if !p.is_null() {
                let head = *(p as *mut *mut UiaUnknownHead);
                ((*head).release)(p);
            }
        }
    }

    /// 持引用的 UIA 元素(FindAll/GetElement 返回即已 AddRef,drop 时 Release)。
    struct UiaElement(*mut c_void);
    impl Drop for UiaElement {
        fn drop(&mut self) {
            unsafe { uia_release(self.0) };
        }
    }

    /// 持引用的元素数组。
    struct UiaElementArray(*mut c_void);
    impl Drop for UiaElementArray {
        fn drop(&mut self) {
            unsafe { uia_release(self.0) };
        }
    }

    /// 持引用的属性条件对象(CreatePropertyCondition 返回已 AddRef)。
    struct UiaCondition(*mut c_void);
    impl Drop for UiaCondition {
        fn drop(&mut self) {
            unsafe { uia_release(self.0) };
        }
    }

    /// 持引用的 IUIAutomation 根对象。
    struct UiaAutomation(*mut c_void);
    impl Drop for UiaAutomation {
        fn drop(&mut self) {
            unsafe { uia_release(self.0) };
        }
    }

    impl UiaAutomation {
        unsafe fn create() -> Option<Self> {
            unsafe {
                let mut p: *mut c_void = std::ptr::null_mut();
                let hr = CoCreateInstance(
                    &CLSID_CUIAUTOMATION,
                    std::ptr::null_mut(),
                    CLSCTX_INPROC_SERVER,
                    &IID_IUIAUTOMATION,
                    &mut p,
                );
                (hr == 0 && !p.is_null()).then(|| Self(p))
            }
        }

        unsafe fn vtbl(&self) -> *mut UiaAutomationVTable {
            unsafe { *(self.0 as *mut *mut UiaAutomationVTable) }
        }

        unsafe fn element_from_handle(&self, hwnd: HWND) -> Option<UiaElement> {
            unsafe {
                let mut el: *mut c_void = std::ptr::null_mut();
                let hr = ((*self.vtbl()).element_from_handle)(self.0, hwnd, &mut el);
                if hr != 0 {
                    // 违约兜底:调用失败却写了出参指针时先释放,防漏。
                    if !el.is_null() {
                        uia_release(el);
                    }
                    return None;
                }
                (!el.is_null()).then(|| UiaElement(el))
            }
        }

        /// 控件类型条件:CreatePropertyCondition(UIA_ControlTypePropertyId, VT_I4)。
        unsafe fn control_type_condition(&self, control_type: i32) -> Option<UiaCondition> {
            unsafe {
                let mut v: VARIANT = std::mem::zeroed();
                VariantInit(&mut v);
                v.Anonymous.Anonymous.vt = VT_I4;
                v.Anonymous.Anonymous.Anonymous.lVal = control_type;
                let mut cond: *mut c_void = std::ptr::null_mut();
                let hr = ((*self.vtbl()).create_property_condition)(
                    self.0,
                    UIA_ControlTypePropertyId,
                    v,
                    &mut cond,
                );
                VariantClear(&mut v);
                (hr == 0 && !cond.is_null()).then(|| UiaCondition(cond))
            }
        }

        /// 字符串属性条件(CreatePropertyCondition + VT_BSTR);BSTR 由 VARIANT
        /// 持有,VariantClear 统一释放。
        unsafe fn string_condition(&self, property_id: i32, value: &str) -> Option<UiaCondition> {
            unsafe {
                let wide: Vec<u16> = value.encode_utf16().collect();
                let bstr = SysAllocStringLen(wide.as_ptr(), wide.len() as u32);
                if bstr.is_null() {
                    return None;
                }
                let mut v: VARIANT = std::mem::zeroed();
                VariantInit(&mut v);
                v.Anonymous.Anonymous.vt = VT_BSTR;
                v.Anonymous.Anonymous.Anonymous.bstrVal = bstr;
                let mut cond: *mut c_void = std::ptr::null_mut();
                let hr = ((*self.vtbl()).create_property_condition)(
                    self.0,
                    property_id,
                    v,
                    &mut cond,
                );
                VariantClear(&mut v);
                (hr == 0 && !cond.is_null()).then(|| UiaCondition(cond))
            }
        }
    }

    impl UiaElement {
        unsafe fn vtbl(&self) -> *mut UiaElementVTable {
            unsafe { *(self.0 as *mut *mut UiaElementVTable) }
        }

        /// 子树内查找首个同条件元素(树序短路;失败 → None)。
        unsafe fn find_first(&self, condition: &UiaCondition) -> Option<UiaElement> {
            unsafe {
                let mut el: *mut c_void = std::ptr::null_mut();
                let hr =
                    ((*self.vtbl()).find_first)(self.0, TreeScope_Descendants, condition.0, &mut el);
                if hr != 0 {
                    if !el.is_null() {
                        uia_release(el);
                    }
                    return None;
                }
                (!el.is_null()).then(|| UiaElement(el))
            }
        }

        /// 子树内查找全部同条件元素(失败/空 → 空数组,不报错)。
        unsafe fn find_all(&self, condition: &UiaCondition) -> Vec<UiaElement> {
            unsafe {
                let mut arr: *mut c_void = std::ptr::null_mut();
                let hr =
                    ((*self.vtbl()).find_all)(self.0, TreeScope_Descendants, condition.0, &mut arr);
                if hr != 0 {
                    if !arr.is_null() {
                        uia_release(arr);
                    }
                    return Vec::new();
                }
                if arr.is_null() {
                    return Vec::new();
                }
                let arr = UiaElementArray(arr);
                let vt = *(arr.0 as *mut *mut UiaElementArrayVTable);
                let mut len: i32 = 0;
                if ((*vt).get_length)(arr.0, &mut len) != 0 || len <= 0 {
                    return Vec::new();
                }
                (0..len)
                    .filter_map(|i| {
                        let mut el: *mut c_void = std::ptr::null_mut();
                        let hr = ((*vt).get_element)(arr.0, i, &mut el);
                        if hr != 0 {
                            if !el.is_null() {
                                uia_release(el);
                            }
                            return None;
                        }
                        (!el.is_null()).then(|| UiaElement(el))
                    })
                    .collect()
            }
        }

        /// 读字符串属性(Name / Value 等);非 BSTR 或失败 → None。
        unsafe fn prop_string(&self, property_id: i32) -> Option<String> {
            unsafe {
                let mut v: VARIANT = std::mem::zeroed();
                VariantInit(&mut v);
                if ((*self.vtbl()).get_current_property_value)(self.0, property_id, &mut v) != 0 {
                    VariantClear(&mut v);
                    return None;
                }
                let out = if v.Anonymous.Anonymous.vt == VT_BSTR {
                    // BSTR 属 VARIANT 所有:只读克隆,由 VariantClear 统一释放。
                    bstr_to_string(v.Anonymous.Anonymous.Anonymous.bstrVal)
                } else {
                    None
                };
                VariantClear(&mut v);
                out
            }
        }
    }

    /// BSTR(*const u16)→ String(空指针 → None;只读克隆,不释放)。
    unsafe fn bstr_to_string(bstr: *const u16) -> Option<String> {
        unsafe {
            if bstr.is_null() {
                return None;
            }
            let mut len = 0usize;
            while len < 4096 && *bstr.add(len) != 0 {
                len += 1;
            }
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(bstr, len)))
        }
    }

    // ---- CLSID/IID(windows-sys 不导出 COM 接口标识,手抄 UIAutomationClient.h) ----
    /// CLSID_CUIAutomation {ff48dba4-60ef-4201-aa87-54103eef594e}
    const CLSID_CUIAUTOMATION: GUID =
        GUID::from_u128(0xff48dba4_60ef_4201_aa87_54103eef594e);
    /// IID_IUIAutomation {30cbe57d-d9d0-452a-ab13-7ac5ac4825ee}
    const IID_IUIAUTOMATION: GUID =
        GUID::from_u128(0x30cbe57d_d9d0_452a_ab13_7ac5ac4825ee);

    // ---- 解析主流程 ----

    /// 入口:光标 → 窗口类分派。任何一步失败 → Unresolvable,绝不 panic。
    pub(super) fn resolve() -> DropTarget {
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            if GetCursorPos(&mut pt) == 0 {
                return DropTarget::Unresolvable("GetCursorPos 失败(无交互桌面?)".into());
            }
            let mut hwnd = WindowFromPoint(pt);
            if hwnd.is_null() {
                return DropTarget::Unresolvable("WindowFromPoint 失败(落点无窗口)".into());
            }
            // 必须取根窗口:桌面图标列表(SysListView32)与 CabinetWClass 的子控件
            // 都会被 WindowFromPoint 直接命中,类名分派只在根窗口上成立。
            let root = GetAncestor(hwnd, GA_ROOT);
            if !root.is_null() {
                hwnd = root;
            }
            let class = window_class(hwnd);
            match class.as_str() {
                "Progman" | "WorkerW" => desktop(),
                "CabinetWClass" => explorer(hwnd),
                other => DropTarget::Unresolvable(format!("不支持的目标窗口类:{other}")),
            }
        }
    }

    /// 窗口类名(失败 → 空串,落入「不支持」桶)。
    unsafe fn window_class(hwnd: HWND) -> String {
        unsafe {
            let mut buf = [0u16; 64];
            let n = GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..n as usize])
        }
    }

    /// 桌面:已知文件夹 Desktop → 校验存在 → Desktop。
    unsafe fn desktop() -> DropTarget {
        unsafe {
            let mut path_pw: *mut u16 = std::ptr::null_mut();
            let hr = SHGetKnownFolderPath(
                &FOLDERID_Desktop,
                0,
                std::ptr::null_mut(),
                &mut path_pw,
            );
            if hr < 0 || path_pw.is_null() {
                return DropTarget::Unresolvable(format!(
                    "SHGetKnownFolderPath(Desktop) 失败 hr={hr:#x}"
                ));
            }
            let s = pwstr_to_string(path_pw);
            CoTaskMemFree(path_pw as *const c_void);
            let Some(s) = s else {
                return DropTarget::Unresolvable("桌面路径解码失败".into());
            };
            let path = PathBuf::from(s);
            if std::fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false) {
                DropTarget::Desktop(path)
            } else {
                DropTarget::Unresolvable(format!("桌面路径不存在:{}", path.display()))
            }
        }
    }

    /// 宽字符串(SHGetKnownFolderPath 分配,调用方 CoTaskMemFree)→ String。
    unsafe fn pwstr_to_string(p: *mut u16) -> Option<String> {
        unsafe {
            if p.is_null() {
                return None;
            }
            let mut len = 0usize;
            while len < 4096 && *p.add(len) != 0 {
                len += 1;
            }
            Some(String::from_utf16_lossy(std::slice::from_raw_parts(p, len)))
        }
    }

    /// 已知文件夹判别值 → SHGetKnownFolderPath 稳定路径(与 desktop() 同一
    /// 调用形态;默认标志 0 + 当前用户,路径串取出后即 CoTaskMemFree)。
    /// 失败 → None,调用方落回既有 Unresolvable 理由,不另立原因文案。
    fn known_folder_path(kf: KnownFolder) -> Option<PathBuf> {
        let fid = match kf {
            KnownFolder::Downloads => FOLDERID_Downloads,
            KnownFolder::Desktop => FOLDERID_Desktop,
            KnownFolder::Documents => FOLDERID_Documents,
            KnownFolder::Pictures => FOLDERID_Pictures,
            KnownFolder::Music => FOLDERID_Music,
            KnownFolder::Videos => FOLDERID_Videos,
        };
        unsafe {
            let mut path_pw: *mut u16 = std::ptr::null_mut();
            let hr = SHGetKnownFolderPath(&fid, 0, std::ptr::null_mut(), &mut path_pw);
            if hr < 0 || path_pw.is_null() {
                return None;
            }
            let s = pwstr_to_string(path_pw);
            CoTaskMemFree(path_pw as *const c_void);
            s.map(PathBuf::from)
        }
    }

    /// 资源管理器窗口:UIA 读地址栏。COM 按本线程 init/uninit 配对;用 MTA:
    /// 解析跑在 spawn_blocking 线程池线程上,STA 没有消息泵、不是跨套 COM 的
    /// 合法居所;MTA 下实测整条 UIA 路径约 70ms。RPC_E_CHANGED_MODE = 线程
    /// 已被初始化成别的套:沿用现套继续干活,不 uninit。
    unsafe fn explorer(hwnd: HWND) -> DropTarget {
        unsafe {
            let hr = CoInitializeEx(std::ptr::null(), COINIT_MULTITHREADED as u32);
            // S_OK(0)/S_FALSE(1) 都要配对 CoUninitialize。
            let need_uninit = hr == 0 || hr == 1;
            if hr < 0 && hr != RPC_E_CHANGED_MODE {
                return DropTarget::Unresolvable(format!("CoInitializeEx 失败 hr={hr:#x}"));
            }
            let out = explorer_walk(hwnd);
            if need_uninit {
                CoUninitialize();
            }
            out
        }
    }

    /// UIA 地址栏走查(候选顺序与降级语义见模块文档)。
    unsafe fn explorer_walk(hwnd: HWND) -> DropTarget {
        unsafe {
            let Some(auto) = UiaAutomation::create() else {
                return DropTarget::Unresolvable("CoCreateInstance(CUIAutomation) 失败".into());
            };
            let Some(root) = auto.element_from_handle(hwnd) else {
                return DropTarget::Unresolvable("UIA ElementFromHandle 失败".into());
            };

            let mut candidates: Vec<String> = Vec::new();
            // 1) 主路:经典地址栏控件(AutomationId 1001,Win7→11 带状结构沿用,
            //    本机 Win11 22000 实测其 Name 即「地址: <完整原始路径>」)。从窗口根
            //    FindFirst 树序短路,顶层 ReBar 带先于文件列表命中,约 35ms。
            let mut address_toolbar: Option<UiaElement> = None;
            if let Some(cond) = auto.string_condition(UIA_AutomationIdPropertyId, "1001")
                && let Some(el) = root.find_first(&cond)
            {
                let class = el.prop_string(UIA_ClassNamePropertyId).unwrap_or_default();
                if class == "ToolbarWindow32" {
                    if let Some(name) = el.prop_string(UIA_NamePropertyId) {
                        candidates.push(name);
                    }
                    address_toolbar = Some(el);
                }
            }
            // 2) 聚焦态补充:用户点进地址栏时出现的 Edit 持有原始路径(非聚焦
            //    态本构建不存在该元素);只扫地址栏子树,开销可忽略。
            if let Some(tb) = address_toolbar.as_ref()
                && let Some(cond) = auto.control_type_condition(UIA_EditControlTypeId)
            {
                for ed in tb.find_all(&cond) {
                    if let Some(v) = ed.prop_string(UIA_ValueValuePropertyId) {
                        candidates.push(v);
                    }
                }
            }
            // 3) 兜底:aid 1001 缺席的异构建型 → 窗口内 ToolbarWindow32 名称全扫
            //    (实测约 0.2s)。
            if candidates.is_empty()
                && let Some(cond) =
                    auto.string_condition(UIA_ClassNamePropertyId, "ToolbarWindow32")
            {
                for tb in root.find_all(&cond) {
                    if let Some(name) = tb.prop_string(UIA_NamePropertyId) {
                        candidates.push(name);
                    }
                }
            }
            // 4) 逐候选解析:剥「地址:」标签 → 盘符绝对路径 → 真实存在的目录;
            //    再退一步认驱动器根显示名「本地磁盘 (C:)」形态。
            for c in &candidates {
                let text = strip_address_label(c).unwrap_or(c.trim());
                if let Some(p) = parse_address_bar(text)
                    && std::fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false)
                {
                    return DropTarget::ExplorerDir(p);
                }
                if let Some(p) = drive_display_root(text)
                    && std::fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false)
                {
                    return DropTarget::ExplorerDir(p);
                }
            }
            // 5) 已知文件夹显示名兜底(r2c):侧栏/快速访问打开的下载/桌面/文档
            //    等,地址栏 Name 只有本地化显示名,4) 全灭;映射命中 →
            //    SHGetKnownFolderPath → 存在性校验 → ExplorerDir。任一步失败
            //    不另立原因,落回下方既有 Unresolvable 文案(原文不变)。
            for c in &candidates {
                let text = strip_address_label(c).unwrap_or(c.trim());
                if let Some(kf) = known_folder_display_path(text)
                    && let Some(p) = known_folder_path(kf)
                    && std::fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false)
                {
                    return DropTarget::ExplorerDir(p);
                }
            }
            let seen = if candidates.is_empty() {
                "未取到任何地址栏候选值".to_string()
            } else {
                candidates.join(" | ")
            };
            DropTarget::Unresolvable(format!("资源管理器地址栏无可落盘路径(候选:{seen})"))
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    /// 六类非文件系统输入(以及相对/盘符相对形态)必须全部拒绝。
    #[test]
    fn parse_address_bar_rejects_non_fs() {
        let bad = [
            // ::{GUID} shell 命名空间(此电脑 / 回收站)
            "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            "::{645FF040-5081-101B-9F08-00AA002F954E}",
            "::{645FF040-5081-101B-9F08-00AA002F954E}\\子目录",
            // shell: 别名
            "shell:RecycleBinFolder",
            "shell:ThisPCFolder",
            // 中文命名空间显示名 / 面包屑形态
            "此电脑",
            "回收站",
            "桌面",
            "图片",
            "此电脑 > 本地磁盘 (C:) > Users",
            // UNC
            "\\\\nas\\share",
            "\\\\nas\\share\\dir",
            // 空 / 空白 / 相对路径 / 盘符相对
            "",
            "   ",
            "C:",
            "C:相对",
            "Users\\admin",
            ".\\x",
            "下载",
        ];
        for t in bad {
            assert!(parse_address_bar(t).is_none(), "应拒绝:{t:?}");
        }
    }

    /// 盘符绝对路径(大小写盘符 / 正反斜杠 / 中文 / 空格 / 尾缀分隔符)全部接受并规整。
    #[test]
    fn parse_address_bar_accepts_plain_paths() {
        let cases: &[(&str, &str)] = &[
            ("C:\\Users\\Administrator\\Desktop", "C:\\Users\\Administrator\\Desktop"),
            ("c:\\users\\admin", "c:\\users\\admin"), // 小写盘符照样接受
            ("C:/Users/admin/文档", "C:\\Users\\admin\\文档"), // 正斜杠 → 反斜杠
            ("C:/", "C:\\"),                           // 正斜杠根 → C:\(保留)
            ("C:\\新建 文件夹\\", "C:\\新建 文件夹"),   // 尾缀分隔符剥除
            ("D:\\", "D:\\"),                          // 根的尾分隔符保留
            ("E:/带 空格/文件 a.txt", "E:\\带 空格\\文件 a.txt"),
            ("  C:\\foo\\bar  ", "C:\\foo\\bar"), // 首尾空白剥除
            ("D:\\中文\\嵌套 子目录\\", "D:\\中文\\嵌套 子目录"),
        ];
        for (input, expect) in cases {
            assert_eq!(
                parse_address_bar(input),
                Some(PathBuf::from(*expect)),
                "输入:{input:?}"
            );
        }
    }

    /// 地址栏标签剥除 + 驱动器根显示名形态。
    #[test]
    fn address_label_and_drive_display() {
        // 标签剥除(半角/全角冒号、大小写英文)
        assert_eq!(strip_address_label("地址: C:\\a\\b"), Some("C:\\a\\b"));
        assert_eq!(strip_address_label("地址:此电脑"), Some("此电脑"));
        assert_eq!(strip_address_label("地址\u{FF1A}此电脑"), Some("此电脑"));
        assert_eq!(strip_address_label("地址\u{FF1A} C:\\x"), Some("C:\\x"));
        assert_eq!(strip_address_label("Address: C:\\a"), Some("C:\\a"));
        assert_eq!(strip_address_label("address:D:\\x"), Some("D:\\x"));
        assert_eq!(strip_address_label("地址:  C:\\x"), Some("C:\\x"));
        assert_eq!(strip_address_label("C:\\无标签"), None);
        // 驱动器根显示名(仅 parse 拒绝整个候选后才尝试)
        assert_eq!(drive_display_root("本地磁盘 (C:)"), Some(PathBuf::from("C:\\")));
        assert_eq!(drive_display_root("OS (D:)"), Some(PathBuf::from("D:\\")));
        assert_eq!(drive_display_root("Windows (e:)"), Some(PathBuf::from("e:\\")));
        assert_eq!(drive_display_root("此电脑"), None);
        assert_eq!(drive_display_root("C:\\a\\b"), None);
        assert_eq!(drive_display_root("普通名字 (无盘符)"), None);
        assert_eq!(drive_display_root("深目录 (C:) 尾巴"), None);
        assert_eq!(drive_display_root("(C:)"), None);
    }

    /// 已知文件夹显示名映射:zh/en 十二名全命中;近似名/小写英文/空串/全
    /// 路径/其他命名空间名全拒;「地址:」标签剥除后的文本流入(管线内先
    /// strip 再映射,即冒烟实测的原始 Name 形态「地址: 下载」)。
    #[test]
    fn known_folder_display_mapping() {
        let hits = [
            ("下载", KnownFolder::Downloads),
            ("Downloads", KnownFolder::Downloads),
            ("桌面", KnownFolder::Desktop),
            ("Desktop", KnownFolder::Desktop),
            ("文档", KnownFolder::Documents),
            ("Documents", KnownFolder::Documents),
            ("图片", KnownFolder::Pictures),
            ("Pictures", KnownFolder::Pictures),
            ("音乐", KnownFolder::Music),
            ("Music", KnownFolder::Music),
            ("视频", KnownFolder::Videos),
            ("Videos", KnownFolder::Videos),
        ];
        for (name, kf) in hits {
            assert_eq!(known_folder_display_path(name), Some(kf), "应命中:{name:?}");
        }
        // 精确匹配面:近似名 / trim 后小写英文 / 空串 / 全路径 / 命名空间名
        assert_eq!(known_folder_display_path("下载2"), None);
        assert_eq!(known_folder_display_path("downloads "), None);
        assert_eq!(known_folder_display_path(""), None);
        assert_eq!(known_folder_display_path("C:\\Users\\admin\\Downloads"), None);
        assert_eq!(known_folder_display_path("此电脑"), None);
        // 管线同款:先剥「地址:」标签再映射
        assert_eq!(
            known_folder_display_path(strip_address_label("地址: 下载").unwrap_or("")),
            Some(KnownFolder::Downloads)
        );
        // trim 生效:带首尾空白的显示名照样命中
        assert_eq!(known_folder_display_path("  下载  "), Some(KnownFolder::Downloads));
    }

    /// Task 12 的 JSON 契约形状。
    #[test]
    fn drop_target_json_shape() {
        let d = drop_target_to_json(&DropTarget::Desktop(PathBuf::from("C:\\dt")));
        assert_eq!(d["kind"], "desktop");
        assert_eq!(d["path"], "C:\\dt");
        assert!(d["reason"].is_null());

        let dir = drop_target_to_json(&DropTarget::ExplorerDir(PathBuf::from("C:\\dir")));
        assert_eq!(dir["kind"], "dir");
        assert_eq!(dir["path"], "C:\\dir");
        assert!(dir["reason"].is_null());

        let un = drop_target_to_json(&DropTarget::Unresolvable("样本原因".into()));
        assert_eq!(un["kind"], "unresolvable");
        assert!(un["path"].is_null());
        assert_eq!(un["reason"], "样本原因");
    }

    /// 手动探针(spike 判据):鼠标悬停到目标(桌面 / 普通资源管理器窗口 /
    /// 库视图 / 此电脑 / 回收站 / 其他应用窗)后,显式运行:
    /// `cargo test --manifest-path ui/src-tauri/Cargo.toml dropresolve -- --ignored --nocapture`
    /// 输出 10 次解析结果,人核;≥9/10 正确 → 窗口落点留 V1。
    #[test]
    #[ignore = "手动探针:先把鼠标移到目标窗口上再运行,输出人工核对"]
    fn manual_probe_prints_targets() {
        let mut desktop = 0;
        let mut dir = 0;
        let mut unresolvable = 0;
        for i in 0..10 {
            let t = resolve_drop_at_cursor();
            match &t {
                DropTarget::Desktop(_) => desktop += 1,
                DropTarget::ExplorerDir(_) => dir += 1,
                DropTarget::Unresolvable(_) => unresolvable += 1,
            }
            println!("[{i}/10] {t:?}");
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        println!("小结:desktop={desktop} dir={dir} unresolvable={unresolvable}");
    }
}
