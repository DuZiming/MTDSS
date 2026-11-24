/// 行内缩略图(V4-1 T19 spec §8):文件行回环口真缩略图,失败/空回退
/// 类型图标(EntryIcon,icons.tsx 存档笔画)。
///
/// 取数面:preview_open(Tauri command 桥透传 daemon IPC)→ 127.0.0.1
/// 回环口 `GET /f/{h}/thumb?w=`(鉴权 Bearer 头;`<img>` 直连不吃头,故
/// fetch→blob→objectURL 收口,previewer image.ts 同炉)→ jpeg 字节。
/// 回退面(恒图标,无重试环):preview_open 拒(源打开失败/文件已变更)、
/// fetch 非 2xx(410 句柄过期等)、**200 空体**(T7 钉面:无后端/不支持
/// 类型恒 200 空字节)、`<img>` onError(CSP 回环面已 T20 放行,残留失败面
/// =句柄竞窗等,图标打底属设计降级非故障)。
///
/// 尝试面:仅**图片/视频**扩展名类(mockup 钉面:文档/压缩包/音频=类型
/// 淡染页形,不发 IPC)且 device 在场;目录恒图标。
///
/// 懒加载 = IntersectionObserver(spec §8 钉面):进入视口附近才发起
/// preview_open IPC 与取数——每行缩略图都是一次 IPC 往返+一次回环请求,
/// 原生 `loading="lazy"` 只推迟 `<img>` 的 src 加载,拦不住先行发生的
/// open+fetch(且 src 本就经 blob 二段装配),故取观察器门。rootMargin
/// 200px 预取带,首触即解订(一次性)。观察器构造经 ioCtor 缝注入
/// (仓库测试环境无布局引擎,假观察器直触发)。
///
/// 生命周期:代次守卫(gen 房法沿 T17)弃迟火;卸载/换参 abort 在途
/// fetch、迟到 open 即刻 close 收口(句柄引用计数不泄漏);objectURL
/// 换面即回收旧值、真卸载时回收现值。
import { useEffect, useRef, useState } from "react";

import type { FsEntry } from "../lib/browse";
import { previewClose, previewOpen } from "../lib/daemon";
import { fileTypeTintKey } from "../lib/filetype";

import { EntryIcon } from "./FileList";

/// 视口外预取带(rootMargin):滚动接近即开装,首屏不闪图标。
const THUMB_ROOT_MARGIN = "200px";

/// 该行是否值得尝试缩略图(见模块注尝试面)。
function thumbWanted(entry: FsEntry, device: string | null): boolean {
  if (entry.is_dir || device == null) return false;
  const tint = fileTypeTintKey(entry.name, false);
  return tint === "img" || tint === "vid";
}

export default function EntryThumb({
  entry,
  device,
  w,
  iconSize,
  ioCtor,
}: {
  entry: FsEntry;
  /// 条目所属设备(preview_open device_id;null = 不尝试缩略图)。
  device: string | null;
  /// 请求缩略图宽(/thumb?w=;调用方按尺寸档传入)。
  w: number;
  /// 回退类型图标的渲染像素(field r19 #1:按档真实尺寸渲染——16px 属性
  /// 被 CSS 放大数倍,笔画亚像素走样=「平铺放大后模糊」;缺省 16 旧行为)。
  iconSize?: number;
  /// IntersectionObserver 构造器缝(缺省全局;测试注入假观察器)。
  ioCtor?: typeof IntersectionObserver;
}) {
  const wanted = thumbWanted(entry, device);
  /// 装载成功面(blob objectURL;null = 未得/回退)。
  const [url, setUrl] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const hostRef = useRef<HTMLSpanElement | null>(null);
  /// 代次:卸载/换参翻代,迟火一律弃。
  const genRef = useRef(0);
  /// 在用 objectURL 记账(换面回收旧值;真卸载回收现值)。
  const urlRef = useRef<string | null>(null);

  // 真卸载回收字节桥(空依赖:dep 变更的重挂不在此收,换面由 adopt 回收)。
  useEffect(() => {
    return () => {
      if (urlRef.current != null) URL.revokeObjectURL(urlRef.current);
    };
  }, []);

  // 装载效应:观察器门 → open → fetch → blob。entry 三元组(path/mtime/
  // size)与 device/w 任一变即重装(重新浏览取新快照/换档升清)。
  useEffect(() => {
    if (!wanted || device == null) return;
    const gen = ++genRef.current;
    const ctrl = new AbortController();
    let handle: string | null = null;
    let started = false;
    const adopt = (next: string): void => {
      if (urlRef.current != null && urlRef.current !== next) {
        URL.revokeObjectURL(urlRef.current);
      }
      urlRef.current = next;
      setUrl(next);
      setFailed(false);
    };
    const run = async (): Promise<void> => {
      if (started) return;
      started = true;
      try {
        const info = await previewOpen(device, entry.path, entry.mtime_ms, entry.size);
        if (gen !== genRef.current) {
          // 迟到的 open:句柄即刻收口,零消费
          void previewClose(info.handle).catch(() => {});
          return;
        }
        handle = info.handle;
        const res = await fetch(
          `http://127.0.0.1:${info.port}/f/${info.handle}/thumb?w=${w}`,
          { headers: { Authorization: `Bearer ${info.token}` }, signal: ctrl.signal },
        );
        if (!res.ok) throw new Error(`thumb HTTP ${res.status}`);
        const bytes = new Uint8Array(await res.arrayBuffer());
        if (gen !== genRef.current) return;
        if (bytes.length === 0) throw new Error("thumb empty"); // T7 空体语义
        adopt(URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" })));
      } catch {
        // open 拒/fetch 败/abort/空体 一律回退;abort 面代次已翻,不落态
        if (gen === genRef.current) setFailed(true);
      } finally {
        if (handle != null) void previewClose(handle).catch(() => {});
      }
    };
    const host = hostRef.current;
    const Ctor =
      ioCtor ??
      (globalThis as { IntersectionObserver?: typeof IntersectionObserver })
        .IntersectionObserver;
    if (Ctor == null || host == null) {
      // 无观察器环境(极端):退化为直接装载(正确性优先于惰性)
      void run();
    } else {
      const io = new Ctor((es) => {
        if (es.some((e) => e.isIntersecting)) {
          io.disconnect();
          void run();
        }
      }, { rootMargin: THUMB_ROOT_MARGIN });
      io.observe(host);
    }
    return () => {
      genRef.current += 1;
      ctrl.abort();
    };
  }, [wanted, device, entry.path, entry.mtime_ms, entry.size, w, ioCtor]);

  return (
    <span className="tile-thumb" ref={hostRef}>
      {url != null && !failed ? (
        <img
          src={url}
          alt=""
          draggable={false}
          // 加载失败(句柄竞窗等;CSP 回环面已 T20 放行):退图标
          onError={() => setFailed(true)}
        />
      ) : (
        <EntryIcon entry={entry} size={iconSize} />
      )}
    </span>
  );
}
