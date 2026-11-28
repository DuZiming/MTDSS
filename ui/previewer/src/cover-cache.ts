/// 封面持久缓存(r45 用户裁定「优化取首页作为缩略图,持久化落地更重要,
/// 不然每次都得去临时取封面」):IndexedDB 存 PDF 首页渲染件(JPEG)。
///
/// - **为什么 IndexedDB**:五端 WebView 通用(WebView2/WebKit/Android);
///   配额充裕(封面 JPEG 几十 KB × 数百,localStorage 5MB 会爆);异步
///   不卡 UI;持久跨会话(杀 App 重启仍命中)。
/// - **键** = 调用方给的稳定文件键(桌面/iOS boot 携 `path|mtime|size`;
///   键缺席 = 跳过缓存,不误存)。文件变更 → mtime 变 → 键变 → 自动
///   失效重渲。
/// - **单 scheme**:渲染仍 pdf.js(cover.ts 唯一渲染器),本件只管存取
///   ——不引入第二渲染路径(用户裁定「做好一套方案」)。
/// - 容错纪律:IndexedDB 不可用(隐私模式/配额)→ 恒 miss 恒跳过,
///   封面退回现渲现状,绝不因缓存层挂掉封面。
const DB_NAME = "mtdss-previewer";
const STORE = "covers";

let dbPromise: Promise<IDBDatabase | null> | null = null;

function openDb(): Promise<IDBDatabase | null> {
  if (dbPromise !== null) return dbPromise;
  dbPromise = new Promise((resolve) => {
    try {
      const req = indexedDB.open(DB_NAME, 1);
      req.onupgradeneeded = () => {
        req.result.createObjectStore(STORE);
      };
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
      req.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
  return dbPromise;
}

/// 取缓存封面(dataURL;miss/不可用 = null,调用方现渲)。
export async function coverGet(key: string): Promise<string | null> {
  const db = await openDb();
  if (db === null) return null;
  return new Promise((resolve) => {
    try {
      const tx = db.transaction(STORE, "readonly");
      const req = tx.objectStore(STORE).get(key);
      req.onsuccess = () => resolve((req.result as string | undefined) ?? null);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
}

/// 存封面(dataURL;失败静默——缓存层不阻塞渲染路径)。
export async function coverPut(key: string, dataUrl: string): Promise<void> {
  const db = await openDb();
  if (db === null) return;
  await new Promise<void>((resolve) => {
    try {
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(dataUrl, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => resolve();
      tx.onabort = () => resolve();
    } catch {
      resolve();
    }
  });
}
