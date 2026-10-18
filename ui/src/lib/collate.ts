/// 字符串比较器:设备名/文件名统一排序入口。
///
/// 确定性纪律(derived.ts 历史):locale 相关 collation 结果随环境变
/// (本机 Windows zh 按拼音,CI en-US 按码点),因此精确顺序断言必须
/// 注入 codePointCmp(UTF-16 码元序,跨环境确定);zhCmp() 供真实 UI
/// 默认使用,其测试只断言任意正确 collator 都满足的性质(自反/反对称/
/// ASCII 字典序),绝不断言具体 CJK 顺序。

/// 比较器契约:负/零/正,与 Array.prototype.sort 兼容。
export type Cmp = (a: string, b: string) => number;

/// UTF-16 码元序:跨环境确定(ASCII 即字典序),测试与需要稳定序处的基准。
export const codePointCmp: Cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/// 中文(拼音)collation 比较器;numeric:true 让 "2" < "10"。
/// 构造 Intl.Collator 异常(能力缺失)时回退码元序——序可能退化但调用永不抛。
export function zhCmp(): Cmp {
  try {
    const collator = new Intl.Collator("zh", { numeric: true });
    return (a, b) => collator.compare(a, b);
  } catch {
    return codePointCmp;
  }
}
