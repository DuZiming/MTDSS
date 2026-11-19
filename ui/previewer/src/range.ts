/// Range 响应头解析(pdf/text 查看器共面):Content-Range「bytes b-e/total」→ 总长。
/// total 为「*」(未知)或缺头 → null;查看器以 null 走「无总长」臂(整取/续读钮常在)。

export function parseRangeTotal(header: string | null): number | null {
  if (header === null) return null;
  const m = /bytes \d+-\d+\/(\d+|\*)/i.exec(header);
  if (m === null) return null;
  return m[1] === "*" ? null : Number(m[1]);
}
