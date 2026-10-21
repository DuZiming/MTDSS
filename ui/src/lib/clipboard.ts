/// 内部剪贴板与粘贴/拖放分派(Task 12 纯逻辑面)。
///
/// 职责边界:本模块只做「种类判定 + 参数装配」的纯函数,不触碰 daemon/
/// 桥;真正发 RPC(transfer.start / move / copy)与冲突预扫/对话框在
/// App 层接线(见 dispatchPaste 系列)。四态语义(spec §4.2,钉死):
/// - 跨设备拖 = 复制、同设备拖 = 移动、任何设备 → 本机目录 = 落盘;
///   修饰键不改义(拖拽手势从不看 Ctrl/Shift)。
/// - MTDSS 复制**不写系统剪贴板**(MTDSS → 真实资源管理器粘贴为 V1
///   反目标);反向(Explorer 复制 → MTDSS 粘贴)由空内部剪贴板时桥
///   `clipboard_files()` 直读 CF_HDROP 落 upload。
///
/// 分派表(pastePlan,目标恒为目录位;selfDeviceId 为本机设备 id):
/// | 源设备      | 目标设备     | kind         | 传输面                    |
/// |-------------|--------------|--------------|---------------------------|
/// | 本机        | 远端         | upload       | transfer.start(网络上传) |
/// | 本机        | 本机         | local_copy   | transfer.start(本机复制  |
/// |             |              |              | 作业卡;M4b 回迁,带策略)|
/// | 远端 A      | 本机         | download     | transfer.start(A 直传本机)|
/// | 远端 A      | 同一台 A     | device_copy  | ctrl copy/move(无网络)  |
/// | 远端 A      | 远端 B       | remote_copy  | transfer.start(A→B 直传, |
/// |             |              |              | 数据不经本机落地)         |

/// 内部剪贴板引用面(brief 钉死最小面 {device, paths, cut})。
/// App 层另持富状态(entries 含 name/is_dir/size/mtime 元数据,供冲突
/// 预扫),派发前投影为本接口。
export interface ClipRef {
  /// 源设备 id(条目坐标系的设备)。
  device: string;
  /// 源绝对路径(该设备坐标系)。
  paths: string[];
  /// 剪切标志(粘贴成功后删源;同设备面决定走 move 还是 copy 语义)。
  cut: boolean;
}

/// 粘贴/拖放分派计划:kind 与 daemon IPC 契约逐字对齐(transfer.start
/// 四形态 + ctrl 同机 copy/move)。同设备两臂:本机→本机 = local_copy
/// (transfer.start 策略化作业面,M4b 回迁);远端 A→A = device_copy
/// (ctrl copy/move,daemon 侧 device==self 短路直调本地 fs)。
export type PastePlan =
  | { kind: "upload"; files: string[]; to: string; dir: string; cut: boolean }
  | { kind: "download"; device: string; paths: string[]; dir: string; cut: boolean }
  | { kind: "remote_copy"; from: string; paths: string[]; to: string; dir: string; cut: boolean }
  | { kind: "device_copy"; device: string; paths: string[]; dir: string; cut: boolean }
  /// 本机→本机(M4b 移交债①回迁;无 device 字段——local_copy 形态本机
  /// 不言自明,策略 skip/overwrite 由执行期冲突流决定后在 App 层补齐)。
  | { kind: "local_copy"; paths: string[]; dir: string; cut: boolean }
  /// 空内部剪贴板:调用方应转读系统剪贴板(CF_HDROP → upload)或直接放弃。
  | { kind: "none"; reason: "empty" };

/// 粘贴分派(纯函数):见模块文档分派表。clip 为 null/空 → none/empty。
/// targetDir 为目标目录的绝对路径(设备根卷列表位 path="" 不是合法传输
/// 目标,由调用方在进入本函数前门控)。
export function pastePlan(
  clip: ClipRef | null,
  selfDeviceId: string | null,
  targetDevice: string,
  targetDir: string,
): PastePlan {
  if (clip == null || clip.paths.length === 0) {
    return { kind: "none", reason: "empty" };
  }
  const { device, paths, cut } = clip;
  // 目标本机:源本机 → local_copy(transfer.start 本机复制作业卡;M4b
  // 移交债①回迁——作业面自带 skip/overwrite 策略参数后,Defect C 的
  // 「冲突对话框不可达」根因消除,本机粘贴重获卡片/进度/跳过计数;M4a
  // 曾临时改派 device_copy 规避);源远端 → download(A 直传本机)。
  if (selfDeviceId != null && targetDevice === selfDeviceId) {
    if (selfDeviceId === device) {
      return { kind: "local_copy", paths, dir: targetDir, cut };
    }
    return { kind: "download", device, paths, dir: targetDir, cut };
  }
  // 目标远端:源本机 → upload;源同一台 → device_copy(ctrl copy/move);
  // 其余 → remote_copy(A→B 直传,数据不经本机)。
  if (selfDeviceId != null && device === selfDeviceId) {
    return { kind: "upload", files: paths, to: targetDevice, dir: targetDir, cut };
  }
  if (device === targetDevice) {
    return { kind: "device_copy", device, paths, dir: targetDir, cut };
  }
  return { kind: "remote_copy", from: device, paths, to: targetDevice, dir: targetDir, cut };
}

/// 窗口内拖放下发分派(纯函数,与 pastePlan 同一判定核心):拖拽语义
/// spec 钉死——同设备拖 = 移动(cut=true)、跨设备拖 = 复制(cut=false)、
/// 任何设备 → 本机目录 = 落盘(download,cut=false 不删远端源);修饰键
/// 不改义。实现即「按设备关系派生 cut 标志后交 pastePlan」,保证拖放与
/// 粘贴两条入口永不漂移出两套分派。
export function dragPlan(
  srcDevice: string,
  paths: string[],
  selfDeviceId: string | null,
  targetDevice: string,
  targetDir: string,
): PastePlan {
  return pastePlan(
    { device: srcDevice, paths, cut: srcDevice === targetDevice },
    selfDeviceId,
    targetDevice,
    targetDir,
  );
}

/// 取路径最后一段(跨平台:反斜杠/正斜杠/混合都认;根形态 → "")。
/// 同设备剪切拼 move 对的目标名用;Windows 盘根 "C:\" 与 POSIX 根
/// "/" 均无最后一段。
export function baseName(path: string): string {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return i === -1 ? path : path.slice(i + 1);
}

/// 按目标目录自身的分隔符拼接子路径(不重复尾分隔符):
/// Windows 目录含 "\" → "C:\Users\x" + "a.txt" = "C:\Users\x\a.txt";
/// 根形 "C:\" / "/" / 带尾分隔符目录不再叠加。同设备剪切面(device_copy
/// 的 ctrl move 对 / local_copy 的策略判定键)共用,与引擎 policy_key
/// 同规则——to = joinDirName(dir, baseName(from))。
export function joinDirName(dir: string, name: string): string {
  const sep = dir.includes("\\") ? "\\" : "/";
  // 根形("C:\"、"/")与已带尾分隔符的目录:直接拼,不重复分隔符。
  return dir.endsWith(sep) || dir === "" ? dir + name : dir + sep + name;
}
