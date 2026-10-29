/// 文件类型词表(纯逻辑):扩展名 → zh 类型显示名 + 行图标 tint 取色键。
/// zh 文案单源于本文件(spec §8 裁定:不入 zh.json;zh.json 旧键
/// files.type.dir/file 由后续任务退役,此处不消费)。

/// 行图标类型角标取色键:策展类别各占一色;Word/音频无专属角标色、
/// 未知与无扩展名归 generic(tint 用 --muted 系);目录归 folder。
export type FileTypeTintKey =
  | "folder"
  | "pdf"
  | "xls"
  | "vid"
  | "exe"
  | "txt"
  | "img"
  | "archive"
  | "generic";

/// 词表类别:显示名 + 取色键 + 该类扩展名(存小写,匹配时同样归小写)。
interface TypeVocab {
  label: string;
  tint: FileTypeTintKey;
}

/// 策展映射(spec §8 基线,扩展名按类别横排,均为小写)。
const CATEGORIES: readonly { label: string; tint: FileTypeTintKey; exts: readonly string[] }[] = [
  { label: "PDF 文档", tint: "pdf", exts: ["pdf"] },
  { label: "Excel 工作表", tint: "xls", exts: ["xls", "xlsx", "csv"] },
  { label: "Word 文档", tint: "generic", exts: ["doc", "docx"] },
  { label: "视频", tint: "vid", exts: ["mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "flv", "mpg", "mpeg", "3gp"] },
  { label: "音频", tint: "generic", exts: ["mp3", "flac", "wav", "aac", "ogg", "m4a", "wma", "opus"] },
  { label: "图片", tint: "img", exts: ["jpg", "jpeg", "png", "gif", "bmp", "webp", "heic", "svg", "tif", "tiff", "ico", "avif"] },
  { label: "压缩包", tint: "archive", exts: ["zip", "7z", "rar", "tar", "gz", "tgz", "bz2", "xz", "zst"] },
  { label: "应用程序", tint: "exe", exts: ["exe", "msi", "bat", "cmd"] },
  { label: "文本文档", tint: "txt", exts: ["txt", "md", "log"] },
];

/// 扩展名 → 类别(小写键);模块加载时一次摊平,查询 O(1)。
const EXT_VOCAB = new Map<string, TypeVocab>();
for (const cat of CATEGORIES) {
  const vocab: TypeVocab = { label: cat.label, tint: cat.tint };
  for (const ext of cat.exts) EXT_VOCAB.set(ext, vocab);
}

/// 取小写扩展名:最后一个点之后;无点、点在首位(点文件)或点在末尾
/// → null(视为无扩展名,与文件管理器惯例一致)。
function extOf(name: string): string | null {
  const i = name.lastIndexOf(".");
  if (i <= 0 || i === name.length - 1) return null;
  return name.slice(i + 1).toLowerCase();
}

/// 类型解析共用主干:目录 / 无扩展名 / 未知扩展名 / 策展命中 四分支。
function vocabOf(name: string, isDir: boolean): TypeVocab {
  if (isDir) return { label: "文件夹", tint: "folder" };
  const ext = extOf(name);
  if (ext === null) return { label: "文件", tint: "generic" };
  const hit = EXT_VOCAB.get(ext);
  if (hit !== undefined) return hit;
  return { label: `${ext.toUpperCase()} 文件`, tint: "generic" };
}

/// 类型显示名:目录「文件夹」;策展命中走映射;未知扩展名 →
/// 「EXT 大写 文件」;无扩展名 → 「文件」。sortEntries 的 type 键与
/// 类型列文案同源,保证排序分组与所见列一致。
export function fileTypeName(name: string, isDir: boolean): string {
  return vocabOf(name, isDir).label;
}

/// 行图标类型角标取色键:目录 folder;策展命中取类别 tint;
/// Word/音频、未知与无扩展名 → generic。
export function fileTypeTintKey(name: string, isDir: boolean): FileTypeTintKey {
  return vocabOf(name, isDir).tint;
}
