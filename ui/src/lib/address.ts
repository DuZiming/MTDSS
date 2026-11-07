/// 地址栏编辑态输入解析(P4b Task 4,spec §7 解析规则;纯函数,单测钉死)。
///
/// 输入 → [`AddressTarget`] 五形态:快捷命名空间直达(V3-3 返工 §15.5:
/// fav 混合视图 / favdir 命名空间位)/ 带设备前缀切设备跳转 / 裸设备名进设
/// 备根 / 当前设备路径跳转;解析失败以 error 形态返回(由 App 统一转
/// toast,不在此处做 i18n)。规则:
///
/// 0. **快捷命名空间语法(V3-3 返工 §15.5)**:`[\\]快捷目录\{快捷名}\{子路径}`
///    (前导反斜杠可有可无——镜像设备前缀形的 `\设备名\` 约定,预填形态不带
///    前导)。首段精确等于 [`FAV_NS_LABEL`] 即尝试快捷解析:快捷名按清单
///    **精确匹配**(重名取清单序首个——创建序,切换另有 chip 下拉承担);
///    命中后无子路径段 = 回该快捷混合视图(fav);有子路径段且该快捷**恰一
///    成员** = rel 落该成员下(分隔符归一为成员设备平台原生,双向同形)成
///    favdir;**多成员 = favAmbiguous 拒绝**(无法按地址定成员,toast 引导
///    从导航进入)。快捷名未命中 → **落设备解析**(裁定:名为「快捷目录」
///    的真实设备不因命名空间语法而失联——同名设备裸形/带子路径形照走既有
///    设备分支,无同名设备得 deviceNotFound 同既有约定)。解析在设备前缀形
///    **之前**判定(同首段碰撞时快捷语法优先,裁定)。
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

/// 解析产物:fav = 回某快捷混合视图;favdir = 进某快捷命名空间位(favId
/// + 成员 device/base + 命名空间 rel);nav = 切换到某设备某目录;deviceRoot
/// = 进某设备根(卷列表位);error = 不跳转,code 对应 App 侧 toast 键
/// (name = 未命中/歧义的候选原文,供文案插值)。
export type AddressTarget =
  | { kind: "fav"; favId: string }
  | { kind: "favdir"; favId: string; device: string; base: string; rel: string }
  | { kind: "nav"; device: string; path: string }
  | { kind: "deviceRoot"; device: string }
  | {
      kind: "error";
      code: "deviceNotFound" | "deviceAmbiguous" | "needDeviceOrAbsolute" | "favAmbiguous";
      name?: string;
    };

/// 参与解析的设备最小切片(与 DeviceInfo 字段子集对齐;online 不参与解
/// 析——离线目标的错误由浏览错误态自然呈现,spec §7.3)。
export interface AddressDevice {
  device_id: string;
  name: string | null;
  platform: string;
}

/// 参与快捷命名空间解析的快捷最小切片(与 Favorite 字段子集对齐;清单序
/// = 创建序,重名取首个)。
export interface AddressFavorite {
  id: string;
  name: string;
  members: readonly { device: string; path: string }[];
}

/// 快捷命名空间语法首段标签(spec §15.5 词面;与 zh.json address.favRoot
/// 展示词同源,常量化防漂移——预填与解析共用本值,不另写。
export const FAV_NS_LABEL = "快捷目录";

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

/// 地址栏编辑态输入解析(规则见模块文档)。favorites 缺省空 = 快捷命名
/// 空间语法不参与(纯设备解析,旧调用形态零回归)。
export function parseAddressInput(
  input: string,
  devices: AddressDevice[],
  current: AddressCurrent,
  favorites: readonly AddressFavorite[] = [],
): AddressTarget {
  const s = input.trim();
  if (s === "") {
    if (current == null) return { kind: "error", code: "needDeviceOrAbsolute" };
    return { kind: "nav", device: current.device, path: current.path };
  }

  // 形 0:快捷命名空间语法(§15.5;解析先于设备前缀形——同名碰撞快捷优先,
  // 裁定)。首段精确等于命名空间标签、第二段命中快捷名才消费;否则整体落
  // 设备解析(裸「快捷目录」=同名设备裸形;名未命中=同名设备带子路径形或
  // deviceNotFound,既有约定)。
  if (favorites.length > 0) {
    const segs = s.replace(/^\\+/, "").split(/[\\/]/).filter((g) => g !== "");
    if (segs[0] === FAV_NS_LABEL && segs.length >= 2) {
      const fav = favorites.find((f) => f.name === segs[1]);
      if (fav != null) {
        const rest = segs.slice(2);
        if (rest.length === 0) return { kind: "fav", favId: fav.id };
        if (fav.members.length !== 1) {
          // 多成员无法按地址定成员(裁定):favAmbiguous 拒绝,toast 引导
          // 从导航进入。无成员(0 个)同拒——「恰一成员」才可直达。
          return { kind: "error", code: "favAmbiguous", name: fav.name };
        }
        const m = fav.members[0];
        const sep = nativeSep(devices.find((d) => d.device_id === m.device)?.platform ?? "");
        // 子路径段已按 [\\/] 拆(双向分隔符同认,镜像形 1 归一纪律),以
        // 成员平台原生分隔符重拼;段非空(nav 纪律:空段已滤),rel 恒非空。
        return { kind: "favdir", favId: fav.id, device: m.device, base: m.path, rel: rest.join(sep) };
      }
    }
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

/// favdir 位编辑态预填(V3-3 返工 §15.5;parseAddressInput 快捷语法的生产
/// 侧镜像,单测钉往返):`快捷目录\{快捷名}\{rel 段…}`——命名空间分隔符
/// 恒反斜杠(虚拟语法词面;解析按 [\\/] 双向同认并归一成员平台原生分隔符,
/// 回车即原位往返)。relSegments = pathSegments 分解段(App 下发),空 =
/// 成员根(`快捷目录\{快捷名}` 两段形)。
export function favdirAddressDraft(favName: string, relSegments: readonly string[]): string {
  return [FAV_NS_LABEL, favName, ...relSegments].join("\\");
}
