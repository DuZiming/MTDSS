/// 地址栏编辑态输入解析(P4b Task 4,spec §7 解析规则;纯函数,单测钉死)。
///
/// 输入 → [`AddressTarget`] 三形态:带设备前缀切设备跳转 / 裸设备名进设
/// 备根 / 当前设备路径跳转;解析失败以 error 形态返回(由 App 统一转
/// toast,不在此处做 i18n)。规则:
///
/// 1. trim 后匹配设备前缀形 `^\+([^\\/]+)[\\/](.+)$`(一或多个前导反斜
///    杠,UNC 双反斜杠同形):首段为设备候选,**解析优先级 = 设备 id 精确
///    > 设备名唯一匹配(大小写不敏感)> 多台同名 → deviceAmbiguous;无
///    命中 → deviceNotFound**。命中后余段按目标设备平台归一分隔符;剥去
///    尾随分隔符后为空 = 裸设备名同义(进设备根);否则绝对形(盘符或根
///    前缀)照用(裸盘符形补尾分隔符成盘根:"D:"→"D:\",终审修复波),
///    **相对形在 POSIX 设备补根前缀成绝对**(设备根 "/" 是唯
///    一根,mockup 冒烟例 `\Linux 主机\home\kk` 即此),**Windows 设备保持
///    原样**(根是卷列表、无单一根可拼;坏路径由现有浏览错误态呈现,
///    spec §7.3)。
/// 2. 裸设备名形 `^\+([^\\/]+)[\\/]*$`(无路径段,尾随分隔符可有可无)
///    = 进入该设备根(卷列表位)。
/// 3. 其余视为当前设备路径:分隔符一律归一为该设备平台原生(nav.ts 语
///    义);绝对形照用,**相对形与当前目录拼接**(当前位是卷列表 path=""
///    时拼接结果即输入本身;posix 根 "/" 拼出根前缀)。
/// 4. **设备层(current=null)任何无设备前缀输入(相对或绝对形)一律
///    needDeviceOrAbsolute**;空/纯空白输入:有当前位返回原位(App 侧把
///    空输入当取消、不发导航,此处保总函数语义),设备层同规则 4。
///
/// 设备平台分隔符:platform === "windows" → 反斜杠,其余(含未知/缺失)
/// → 正斜杠。目标平台原生完整路径(Loc.path 契约,nav.ts)恒为绝对;
/// Windows 相对余段是唯一例外,是有意为之(见规则 1)。
///
/// 路径分解纪律(nav.ts):余段的尾随分隔符剥除经由导出的 [`splitPath`]
/// 完成,不在本模块另写解析。
import { splitPath } from "./nav";

/// 解析产物:nav = 切换到某设备某目录;deviceRoot = 进某设备根(卷列表
/// 位);error = 不跳转,code 对应 App 侧 toast 键(name = 未命中/歧义的
/// 设备候选原文,供文案插值)。
export type AddressTarget =
  | { kind: "nav"; device: string; path: string }
  | { kind: "deviceRoot"; device: string }
  | {
      kind: "error";
      code: "deviceNotFound" | "deviceAmbiguous" | "needDeviceOrAbsolute";
      name?: string;
    };

/// 参与解析的设备最小切片(与 DeviceInfo 字段子集对齐;online 不参与解
/// 析——离线目标的错误由浏览错误态自然呈现,spec §7.3)。
export interface AddressDevice {
  device_id: string;
  name: string | null;
  platform: string;
}

/// 解析当前位置:目录位 {device, path};设备层为 null。
export type AddressCurrent = { device: string; path: string } | null;

/// 设备候选 → 设备:先 id 精确,再名唯一(大小写不敏感;null 名按空串),
/// 多台同名返回 "ambiguous",无命中返回 null。
function resolveDevice(
  devices: AddressDevice[],
  candidate: string,
): AddressDevice | "ambiguous" | null {
  const byId = devices.find((d) => d.device_id === candidate);
  if (byId != null) return byId;
  const lower = candidate.toLowerCase();
  const byName = devices.filter((d) => (d.name ?? "").toLowerCase() === lower);
  if (byName.length === 1) return byName[0];
  if (byName.length > 1) return "ambiguous";
  return null;
}

/// 平台原生分隔符:windows → 反斜杠,其余 → 正斜杠。
function nativeSep(platform: string): "\\" | "/" {
  return platform === "windows" ? "\\" : "/";
}

/// 绝对形判定(spec「无盘符/无根前缀」的否定):盘符两平台通认
/// (`X:`),根前缀取目标平台原生分隔符(posix "/"、windows "\")。
function looksAbsolute(p: string, sep: "\\" | "/"): boolean {
  if (/^[A-Za-z]:/.test(p)) return true;
  return p.startsWith(sep);
}

/// 地址栏编辑态输入解析(规则见模块文档)。
export function parseAddressInput(
  input: string,
  devices: AddressDevice[],
  current: AddressCurrent,
): AddressTarget {
  const s = input.trim();
  if (s === "") {
    if (current == null) return { kind: "error", code: "needDeviceOrAbsolute" };
    return { kind: "nav", device: current.device, path: current.path };
  }

  // 形 1:设备前缀(\+候选+分隔符+余段)
  const pre = /^\\+([^\\/]+)[\\/](.+)$/.exec(s);
  if (pre != null) {
    const hit = resolveDevice(devices, pre[1]);
    if (hit === "ambiguous") {
      return { kind: "error", code: "deviceAmbiguous", name: pre[1] };
    }
    if (hit == null) {
      return { kind: "error", code: "deviceNotFound", name: pre[1] };
    }
    const sep = nativeSep(hit.platform);
    // 余段归一为目标平台原生分隔符(nav.ts 语义:严禁以单一 '/' 处理)。
    let rest = pre[2].replace(/[\\/]/g, sep);
    // 尾随分隔符剥除(经 splitPath,纪律见模块文档):仅剩分隔符 = 裸
    // 设备名同义,进设备根。
    rest = splitPath(rest).segs.join(sep);
    // 盘根尾斜杠特例:剥尾分隔符后恰剩裸盘符("D:")是 Windows 驱动器相
    // 对形,补回分隔符成规范盘根("D:\",与 daemon 下发形态一致)。
    if (sep === "\\" && /^[A-Za-z]:$/.test(rest)) {
      rest += "\\";
    }
    if (rest === "") {
      return { kind: "deviceRoot", device: hit.device_id };
    }
    if (!looksAbsolute(rest, sep) && sep === "/") {
      rest = "/" + rest; // POSIX 设备:相对余段补根前缀成绝对
    }
    return { kind: "nav", device: hit.device_id, path: rest };
  }

  // 形 2:裸设备名(\+候选,尾随分隔符可有可无)= 进设备根
  const bare = /^\\+([^\\/]+)[\\/]*$/.exec(s);
  if (bare != null) {
    const hit = resolveDevice(devices, bare[1]);
    if (hit === "ambiguous") {
      return { kind: "error", code: "deviceAmbiguous", name: bare[1] };
    }
    if (hit == null) {
      return { kind: "error", code: "deviceNotFound", name: bare[1] };
    }
    return { kind: "deviceRoot", device: hit.device_id };
  }

  // 形 3:无设备前缀 = 当前设备路径;设备层一律提示
  if (current == null) return { kind: "error", code: "needDeviceOrAbsolute" };
  const curDev = devices.find((d) => d.device_id === current.device);
  const sep = nativeSep(curDev?.platform ?? "");
  const norm = s.replace(/[\\/]/g, sep);
  if (looksAbsolute(norm, sep)) {
    return { kind: "nav", device: current.device, path: norm };
  }
  const base = current.path;
  const path = base === "" || base.endsWith(sep) ? base + norm : base + sep + norm;
  return { kind: "nav", device: current.device, path };
}
