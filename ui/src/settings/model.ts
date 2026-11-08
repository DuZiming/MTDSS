/// 设置面模型层(V3-4 §5:Windows 设置对话框):五字段设置形态与缺省、
/// 信任模式 roots.set 载荷装配、登录链设备名参数——全部纯函数,零 IO,
/// settings.test 直驱钉行为。
///
/// 职责边界:RPC 透传面在 lib/daemon(settingsGet/settingsSet/rootsSet,
/// 本模块只供类型);持久化权威 = 引擎 settings.json(daemon settings.get/
/// settings.set);信任切换的热生效 = ui 装配 roots.set 载荷(收件箱 +
/// 所选目录 / 空集直通)经 Tauri 桥转发(T1 IPC op,免重启)。

/// roots.set 载荷单根(与 daemon need_root_specs 解析面逐字对齐):
/// name 空串 = 对端列表回退末段真实目录名;source "fs" = 真实本机路径;
/// visible = 对端根列表可见性(桌面信任面恒 true)。
export interface RootSpecWire {
  name: string;
  path: string;
  source: "fs" | "bridge";
  visible: boolean;
}

/// 设置面五字段(daemon settings.get 投影键面逐字对齐):三个门默认关、
/// 信任目录空表、设备名 null = 旧行为零回归(删除入口照旧/浏览全量/
/// 授权根 None 直通/登录回落主机名)。
export interface V34Settings {
  allow_remote_delete: boolean;
  show_hidden: boolean;
  trust_mode: boolean;
  trusted_roots: string[];
  device_name: string | null;
}

/// 缺省设置(daemon 侧 Default 的 TS 镜像):设置拉取失败时的兜底面,
/// 与 daemon 缺省同值——门全关 = 桌面既有行为。
export const V34_DEFAULTS: V34Settings = {
  allow_remote_delete: false,
  show_hidden: false,
  trust_mode: false,
  trusted_roots: [],
  device_name: null,
};

/// settings.set 补丁(Partial:缺省键 = daemon 侧不动,update.prefs.set
/// 同款口径)。
export type SettingsPatch = Partial<V34Settings>;

/// 收件箱根的显示名(对端根列表词面;与侧栏/着陆页收件箱行同词)。
const INBOX_ROOT_NAME = "收件箱";

/// 信任模式 roots.set 载荷(纯函数):开 = 收件箱(生效接收目录)+
/// 所选目录(顺序保留);recv_dir 缺失(status 未就绪)不出空路径条目
/// (空路径进守卫必被丢弃,徒增 fail-closed 噪声);关(空表)= 显式
/// 空集 = 守卫直通(T1 op 空集语义,桌面默认恢复)。
export function trustRootsPayload(
  recvDir: string | null,
  trusted: readonly string[],
): RootSpecWire[] {
  const out: RootSpecWire[] = [];
  if (recvDir != null && recvDir !== "") {
    out.push({ name: INBOX_ROOT_NAME, path: recvDir, source: "fs", visible: true });
  }
  for (const dir of trusted) {
    // 名空串 = 守卫回退末段真实目录名(roots_listing 既有语义)。
    out.push({ name: "", path: dir, source: "fs", visible: true });
  }
  return out;
}

/// 登录链设备名参数(设备名迁移,V3-4 §5-4:登录表单去字段,取名
/// settings.device_name):去首尾空白,空 = null(daemon 回落主机名)。
/// 既有语义不变(原表单空串同样传 null)。
export function loginNameArg(name: string | null | undefined): string | null {
  const v = (name ?? "").trim();
  return v === "" ? null : v;
}
