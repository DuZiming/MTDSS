/// 右侧详情栏(field 定稿 2026-11-23:右侧小栏只展示文件缩略图+元数据,
/// 预览主体=弹窗):左侧拖柄(宽 25-70%,kv 键 previewpanel:width,缺省
/// 40%)+ 工具栏(‹› 切换 / 文件名+大小 / ⤓ 下载 / ↗ 用其他应用打开 /
/// ⧉ 打开预览(主 CTA,弹窗承载预览)/ ✕ 关)+ 详情区(缩略图 + 元数据
/// 行:文件名/大小/格式/分辨率/访问·创建·修改时间;时间缺=「—」)。
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

import { fmtSize, type FsEntry } from "../lib/browse";
import { previewClose, previewOpen, type PreviewOpenInfo } from "../lib/daemon";
import { fileTypeName } from "../lib/filetype";
import {
  PANEL_WIDTH_DEFAULT,
  loadPanelWidth,
  panelWidthAt,
  savePanelWidth,
} from "../lib/panelwidth";

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

/// Unix 毫秒 → 本地日期时间;0/缺(旧对端/平台不可得)=「—」。
function fmtDate(ms: number | undefined): string {
  if (ms === undefined || ms === 0) return "—";
  const d = new Date(ms);
  const p = (n: number): string => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

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
  /// 句柄信息(端口+令牌:缩略图/分辨率探读 URL 装配)。
  const [info, setInfo] = useState<PreviewOpenInfo | null>(null);
  /// 图片分辨率(探读;非图=null 不显行)。缩略图取 w=256(与列表行同源)。
  const [resolution, setResolution] = useState<string | null>(null);
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

  // 详情装配效应(field 定稿:开句柄(端口+令牌供缩略图/分辨率探读),
  // 不再挂 previewer——预览主体=⧉ 弹窗)。
  useEffect(() => {
    const gen = ++genRef.current;
    setPhase("loading");
    setInfo(null);
    setResolution(null);
    let handle: string | null = null;
    void (async () => {
      try {
        const opened = await previewOpen(
          source.device,
          source.entry.path,
          source.entry.mtime_ms,
          source.entry.size,
        );
        if (gen !== genRef.current) {
          void previewClose(opened.handle).catch(() => {});
          return;
        }
        handle = opened.handle;
        setInfo(opened);
        setPhase("ready");
        // 分辨率探读(图片:离屏 img 读 natural 尺寸;非图=跳过)
        if (source.entry.size > 0 && /\.(png|jpe?g|gif|bmp|webp|avif|ico|tiff?)$/i.test(source.entry.name)) {
          const probe = new Image();
          probe.onload = (): void => {
            if (gen === genRef.current)
              setResolution(`${probe.naturalWidth} × ${probe.naturalHeight}`);
          };
          probe.onerror = (): void => undefined;
          probe.src = `http://127.0.0.1:${opened.port}/f/${opened.handle}/thumb?w=256&t=${opened.token}`;
        }
      } catch {
        if (gen === genRef.current) setPhase("error");
      }
    })();
    return () => {
      genRef.current += 1;
      if (handle != null) void previewClose(handle).catch(() => {});
    };
  }, [source.device, source.entry.path, source.entry.mtime_ms, source.entry.size]);

  // ---- 左缘拖柄(field 定稿 2026-11-23:px 制,弃百分比——百分比在宽屏
  // 上过大;默认 300px,clamp [240,560],kv 持久沿用)----
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
    aside.style.width = `${w}px`; // 廉价路径:拖拽中不 setState(见上注)
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
      style={{ width: `${width}px` }}
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
      <div className="preview-body preview-details">
        {/* 详情区(field 定稿:缩略图+元数据;预览主体=⧉ 弹窗)。错误面
            覆于其上(open 拒/源打开失败,点列表行即换源再试)。 */}
        {phase === "ready" && info != null ? (
          <>
            <div className="preview-details-thumb">
              <img
                src={`http://127.0.0.1:${info.port}/f/${info.handle}/thumb?w=256&t=${info.token}`}
                alt=""
                draggable={false}
                onError={(e): void => {
                  (e.target as HTMLImageElement).style.visibility = "hidden";
                }}
              />
            </div>
            <div className="preview-details-rows" role="table">
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">文件名</span>
                <span className="preview-details-val" title={source.entry.name}>{source.entry.name}</span>
              </div>
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">大小</span>
                <span className="preview-details-val">{fmtSize(source.entry.size, false)}</span>
              </div>
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">格式</span>
                <span className="preview-details-val">{fileTypeName(source.entry.name, source.entry.is_dir)}</span>
              </div>
              {resolution != null && (
                <div className="preview-details-row" role="row">
                  <span className="preview-details-key">分辨率</span>
                  <span className="preview-details-val">{resolution}</span>
                </div>
              )}
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">修改时间</span>
                <span className="preview-details-val">{fmtDate(source.entry.mtime_ms)}</span>
              </div>
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">创建时间</span>
                <span className="preview-details-val">{fmtDate(source.entry.ctime_ms)}</span>
              </div>
              <div className="preview-details-row" role="row">
                <span className="preview-details-key">访问时间</span>
                <span className="preview-details-val">{fmtDate(source.entry.atime_ms)}</span>
              </div>
            </div>
          </>
        ) : phase === "loading" ? (
          <div className="preview-details-loading" role="status">{t("preview.loading")}</div>
        ) : (
          <div className="preview-failed" role="status">{t("preview.loadFailed")}</div>
        )}
      </div>
    </aside>
  );
}
