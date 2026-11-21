/// 时长格式化单点(V4-1 T21 磁贴时长角标):秒 → `m:ss` / `h:mm:ss`。
/// 消费面 = FsEntry.duration_secs(V4-1 T21 proto 增量;源端机会主义填充,
/// 0 = 未知)——有则显无则不显:非正数/非数一律空串,角标不落垃圾。

/// 秒 → 角标串。`s <= 0` / 非有限数 → ""(未知面,调用方以空串判不显);
/// 小数向下取整;不满 1 小时 = `m:ss`(分位不补零);满 1 小时 = `h:mm:ss`。
export function fmtDuration(s: number): string {
  if (!Number.isFinite(s) || s <= 0) return "";
  const total = Math.floor(s);
  const secs = total % 60;
  const mins = Math.floor(total / 60) % 60;
  const hours = Math.floor(total / 3600);
  const ss = String(secs).padStart(2, "0");
  if (hours === 0) return `${mins}:${ss}`;
  return `${hours}:${String(mins).padStart(2, "0")}:${ss}`;
}
