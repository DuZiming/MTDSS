/// 目录视图模式记忆(V4-1 T19):viewMode 每目录持久化,键值走账号 KV
/// (kv.get/kv.put 透传,沿 favorites 键值先例)。
///
/// 键面(brief 钉死形态 + favorites 复合键先例取舍):
/// - dir 位 `viewmode:{device}:{dir}`——收件箱/根卷都是普通 dir 位,天然
///   覆盖,不另设复合键;
/// - favdir 位(快捷命名空间)V3-3 返工 §13.6 同款取成员**原生路径**
///   (joinRel(base,rel),与浏览管线同源)——同一物理目录经 dir 位与
///   favdir 位两种入口进,记忆同键共享;
/// - fav 位(混合视图)`viewmode:fav:{favId}`——favorites 复合键先例:
///   实体以稳定 id 访名(16 hex,改名不漂移;账号 KV 共享,同一快捷跨端
///   同键,视图记忆随之跨端);
/// - devices/favs 着陆位非目录视图(两视图不消费 viewMode),null=不记忆。
///
/// 读写尽力而为:kvGet 失败(离线/未登录)→ 缺省,浏览不因记忆失败;
/// kvPut 失败 → 静默(会话内切换不受影响,下次进入落上次成功值)。
/// 存值=模式字面量原串,解析白名单收口(未知值 → 缺省,前向兼容)。

import { kvGet, kvPut } from "./daemon";
import { joinRel, type Loc } from "./nav";

/// 内容区视图三态(单源;Toolbar re-export 保既有消费面不动)。
export type ViewMode = "details" | "list" | "tiles";

/// 缺省视图(App viewMode 初值同源,与既有「默认详细信息」一致)。
export const VIEWMODE_DEFAULT: ViewMode = "details";

/// 键前缀。
export const VIEWMODE_KV_PREFIX = "viewmode";

/// 位置 → 记忆键;null = 该位不记忆(见模块注键面)。
export function viewModeKey(loc: Loc): string | null {
  switch (loc.kind) {
    case "dir":
      return `${VIEWMODE_KV_PREFIX}:${loc.device}:${loc.path}`;
    case "favdir":
      return `${VIEWMODE_KV_PREFIX}:${loc.device}:${joinRel(loc.base, loc.rel)}`;
    case "fav":
      return `${VIEWMODE_KV_PREFIX}:fav:${loc.id}`;
    default:
      return null;
  }
}

/// kv 存值 → ViewMode(白名单;null/未知/空串 → 缺省)。
export function parseViewMode(raw: string | null): ViewMode {
  return raw === "details" || raw === "list" || raw === "tiles"
    ? raw
    : VIEWMODE_DEFAULT;
}

/// 读取记忆:键不存在(null)或读取失败(离线/未登录)一律缺省。
export async function loadViewMode(key: string): Promise<ViewMode> {
  try {
    return parseViewMode(await kvGet(key));
  } catch {
    return VIEWMODE_DEFAULT;
  }
}

/// 写入记忆:尽力而为,失败静默(调用方不等待不重试)。
export async function saveViewMode(key: string, mode: ViewMode): Promise<void> {
  try {
    await kvPut(key, mode);
  } catch {
    // 持久化失败不阻断会话内切换
  }
}
