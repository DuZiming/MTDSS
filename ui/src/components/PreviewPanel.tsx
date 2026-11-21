/// 右侧预览面板(V4-1 T20,mockup「桌面 · 右侧预览面板」钉面):左侧
/// 拖柄(宽 25-70%,kv 键 previewpanel:width,缺省 40%)+ 工具栏(‹›
/// 切换 / 文件名+大小 / ⤓ 下载 / ↗ 用其他应用打开 / ⧉ 弹出独立窗 / ✕
/// 关)+ previewer 挂载区(T16-18 bundle 静态 import=本任务接线)。
///
/// 数据面:source 变更 → preview_open(Tauri command 桥透传 daemon IPC;
/// 远端 device_id,本机 null=fs 直读)→ buildPreviewRequest(lib/preview
/// 单点,URL 携 ?t= 查询令牌=裁定②)→ mountPreview。换源/卸载一律
/// destroy 装配 + previewClose 收口(句柄引用计数,EntryThumb 同纪律);
/// 迟到的 open 按代次弃(即刻 close 那枚引用,零泄漏)。
///
/// 交互:Esc 关(document keydown,组件自持);左缘拖柄拖调宽度(sidebar
/// 拖柄同款指针捕获语义,拖拽中直写 DOM 廉价路径,松手才落 state+kv);
/// ⧉ 纯上抛 props.onPopOut(),App 按源现值重开一枚句柄引用、重装配
/// PreviewRequest 起 Tauri WebWindow(窗关即释,面板关即释,多开允许)。
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { mountPreview, type PreviewHandle } from "@mtdss/previewer";
import { fmtSize, type FsEntry } from "../lib/browse";
import { previewClose, previewOpen } from "../lib/daemon";
import {
  PANEL_WIDTH_DEFAULT,
  loadPanelWidth,
  panelWidthAt,
  savePanelWidth,
} from "../lib/panelwidth";
import { buildPreviewRequest } from "../lib/preview";

export interface PreviewSource {
  /// preview_open device_id(null = 本机,fs 直读臂)。
  device: string | null;
  entry: FsEntry;
}

interface PreviewPanelProps {
  source: PreviewSource;
  hasPrev: boolean;
  hasNext: boolean;
  onPrev(): void;
  onNext(): void;
  onClose(): void;
  /// ⤓ 下载(App 既有下载流;App 仅远端源接,本机不渲染下载钮)。
  onDownload(): void;
  canDownload: boolean;
  /// ↗ 用其他应用打开(本机=shell open 原路径;远端=App toast 引导)。
  onOpenExternal(): void;
  /// ⧉ 弹出独立窗(纯上抛;App 按 source 现值重开一枚句柄引用并装配
  /// PreviewRequest,起 Tauri WebWindow——同键再开同句柄,窗关即释)。
  onPopOut(): void;
}

/// 装配态:loading= IPC 往返中;ready= previewer 已挂载;error= open 拒
/// (源打开失败/文件已变更,无重试面——重开即换源再试)。
type PanelPhase = "loading" | "ready" | "error";

export default function PreviewPanel({
  source,
  hasPrev,
  hasNext,
  onPrev,
  onNext,
  onClose,
  onDownload,
  canDownload,
  onOpenExternal,
  onPopOut,
}: PreviewPanelProps) {
  const { t } = useTranslation();
  /// 面板宽(占宿主 .body 百分比;会话态,kv 装载覆盖)。
  const [width, setWidth] = useState(PANEL_WIDTH_DEFAULT);
  const [phase, setPhase] = useState<PanelPhase>("loading");
  const bodyRef = useRef<HTMLDivElement | null>(null);
  const asideRef = useRef<HTMLElement | null>(null);
  /// 代次守卫(T17 房法):弃迟到的 open/mount。
  const genRef = useRef(0);
  /// 拖柄会话档(ref 不触发渲染;sidebar 拖柄同款)。
  const dragRef = useRef<{ pointerId: number; w: number } | null>(null);

  // 装载宽度记忆(尽力而为,失败缺省 40)。
  useEffect(() => {
    let alive = true;
    void loadPanelWidth().then((w) => {
      if (alive) setWidth(w);
    });
    return () => {
      alive = false;
    };
  }, []);

  // Esc 关面板(document 级;组件卸载即摘,不影响独立窗焦点面)。
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  // 预览装配效应:source 四元组任一变即重开重挂(gen 房法弃迟火;
  // 收口面 destroy 装配 + previewClose 引用)。
  useEffect(() => {
    const gen = ++genRef.current;
    setPhase("loading");
    let mounted: PreviewHandle | null = null;
    let handle: string | null = null;
    void (async () => {
      try {
        const info = await previewOpen(
          source.device,
          source.entry.path,
          source.entry.mtime_ms,
          source.entry.size,
        );
        if (gen !== genRef.current) {
          // 迟到的 open:句柄即刻收口,零消费(EntryThumb 同款)
          void previewClose(info.handle).catch(() => {});
          return;
        }
        handle = info.handle;
        const req = buildPreviewRequest(info, source.entry.name, source.entry.size);
        const host = bodyRef.current;
        if (host == null || gen !== genRef.current) return;
        mounted = mountPreview(host, req, () => {});
        if (gen !== genRef.current) {
          mounted.destroy();
          mounted = null;
          return;
        }
        setPhase("ready");
      } catch {
        if (gen === genRef.current) setPhase("error");
      }
    })();
    return () => {
      genRef.current += 1;
      mounted?.destroy();
      if (handle != null) void previewClose(handle).catch(() => {});
    };
  }, [source.device, source.entry.path, source.entry.mtime_ms, source.entry.size]);

  // ---- 左缘拖柄(sidebar 拖柄同款:捕获保证移出命中区仍跟踪;合成
  // 事件无真实指针时捕获抛错,吞掉不影响建档。拖拽中直写 DOM 廉价路径,
  // 松手才 setState+save)----
  function onGripPointerDown(e: React.PointerEvent<HTMLSpanElement>): void {
    if (e.button !== 0 || dragRef.current != null) return;
    e.preventDefault(); // 拖宽不选中文本
    dragRef.current = { pointerId: e.pointerId, w: width };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // 无真实指针(自动化)→ 跳过捕获,后续事件按派发目标照常到达
    }
  }

  function onGripPointerMove(e: React.PointerEvent<HTMLSpanElement>): void {
    const d = dragRef.current;
    const aside = asideRef.current;
    const host = aside?.parentElement;
    if (d == null || aside == null || host == null) return;
    const rect = host.getBoundingClientRect();
    const w = panelWidthAt(e.clientX, rect.right, rect.width);
    d.w = w;
    aside.style.width = `${w}%`; // 廉价路径:拖拽中不 setState(见上注)
  }

  function onGripPointerUp(e: React.PointerEvent<HTMLSpanElement>): void {
    const d = dragRef.current;
    if (d == null) return;
    dragRef.current = null;
    if (e.currentTarget.hasPointerCapture(e.pointerId)) {
      e.currentTarget.releasePointerCapture(e.pointerId);
    }
    setWidth(d.w);
    void savePanelWidth(d.w); // 写失败静默:会话内已生效,仅不跨会话记忆
  }

  const nameLine = `${source.entry.name} · ${fmtSize(source.entry.size, false)}`;

  return (
    <aside
      ref={asideRef}
      className="preview-panel"
      style={{ width: `${width}%` }}
      aria-label={t("preview.title")}
    >
      <span
        className="preview-grip"
        role="separator"
        aria-orientation="vertical"
        aria-label={t("preview.resize")}
        onPointerDown={onGripPointerDown}
        onPointerMove={onGripPointerMove}
        onPointerUp={onGripPointerUp}
        onPointerCancel={onGripPointerUp}
      />
      <div className="preview-toolbar">
        <button
          type="button"
          className="preview-prev"
          aria-label={t("preview.prev")}
          title={t("preview.prev")}
          disabled={!hasPrev}
          onClick={onPrev}
        >
          ‹
        </button>
        <button
          type="button"
          className="preview-next"
          aria-label={t("preview.next")}
          title={t("preview.next")}
          disabled={!hasNext}
          onClick={onNext}
        >
          ›
        </button>
        <span className="preview-name" title={nameLine}>
          {nameLine}
        </span>
        {canDownload && (
          <button
            type="button"
            className="preview-download"
            aria-label={t("preview.download")}
            title={t("preview.download")}
            onClick={onDownload}
          >
            ⤓
          </button>
        )}
        <button
          type="button"
          className="preview-openext"
          aria-label={t("preview.openWith")}
          title={t("preview.openWith")}
          onClick={onOpenExternal}
        >
          ↗
        </button>
        <button
          type="button"
          className="preview-popout"
          aria-label={t("preview.popOut")}
          title={t("preview.popOut")}
          onClick={onPopOut}
        >
          ⧉
        </button>
        <button
          type="button"
          className="preview-close"
          aria-label={t("preview.close")}
          title={t("preview.close")}
          onClick={onClose}
        >
          ✕
        </button>
      </div>
      <div className="preview-body">
        {/* 挂载区恒在场(error 态不摘——bodyRef 引用稳定,换源重挂零竞);
            previewer 接管后 loading 态由各查看器自带加载面,换源时 destroy
            即摘旧装配,无残留。错误面覆于其上(装配即不可预览:open 拒/
            源打开失败,无重试面——点列表行即换源再试)。 */}
        <div ref={bodyRef} className="preview-mount" />
        {phase === "error" && (
          <div className="preview-failed" role="status">
            {t("preview.loadFailed")}
          </div>
        )}
      </div>
    </aside>
  );
}
