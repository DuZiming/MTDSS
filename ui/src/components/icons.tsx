/// 内联 SVG 图标组件库(P4b Task 3):笔画逐值转录自定稿 mockup 存档
/// (brainstorm 2186-1793238382:linear 族与 folder/file 页形出自
/// visual-style-v2.html 的 const L/folder()/filetint();方向与抽屉操作
/// 出自 visual-style-v4/v6.html 的 const DIR/const L),不自创笔画。
///
/// 纪律(设计 §4):本文件零字面色——线性族 stroke=currentColor 随文本
/// 色;着色族 fill/stroke 只引 themes.css 的 --icon-* 变量。尺寸统一
/// 单 prop `size`(存档 svg 封装默认 15;IcChevronDown 存档 12,故默认
/// 12);viewBox 恒 0 0 16 16,aria-hidden(纯装饰,语义由外层按钮/
/// 文案承载)。后续任务按导出名消费,勿改导出面(合同见 icons.test.tsx)。
import type { ReactNode } from "react";

import type { FileTypeTintKey } from "../lib/filetype";

/// 着色族 tint 取色键:镜像 filetype.ts 的 FileTypeTintKey 去掉
/// "folder"(目录走 IcFolder,不经 tint 路径,后续任务负责分支);
/// "generic"(Word/音频/未知/无扩展名)无专属 tint 变量,裁定取页缘
/// var(--icon-file-edge) 作角标底色(与页身同族的中性淡化,三主题均成立)。
export type IconTintKey = Exclude<FileTypeTintKey, "folder">;

/// 线性族根属性:存档 const S 逐值(stroke-width 1.4、圆端/圆拐、无填充)。
/// SVG 呈现属性自 svg 根继承,子元素无需逐一复写;个别实心元素(theme
/// 三点)以 fill="currentColor" 就地覆盖。
const S = {
  stroke: "currentColor",
  strokeWidth: 1.4,
  strokeLinecap: "round",
  strokeLinejoin: "round",
  fill: "none",
} as const;

function Icon({ size = 15, children }: { size?: number; children: ReactNode }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true" {...S}>
      {children}
    </svg>
  );
}

/// 着色族根封装:存档 folder()/filetint() 的 svg 封装不带 S(无描边属性
/// → stroke-width 恒 1、直角连接),与线性族(1.4 圆角)刻意不同,此为
/// 存档原貌,勿统一。fill/stroke 由子元素各落主题变量。
function TintIcon({ size = 15, children }: { size?: number; children: ReactNode }) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" aria-hidden="true">
      {children}
    </svg>
  );
}

/* ---- 导航 -------------------------------------------------------- */

export function IcBack({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M9.5 3.5 5 8l4.5 4.5" />
    </Icon>
  );
}

export function IcForward({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M6.5 3.5 11 8l-4.5 4.5" />
    </Icon>
  );
}

export function IcUp({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M8 13V3.5M4.5 7 8 3.5 11.5 7" />
    </Icon>
  );
}

export function IcChevronRight({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M6 3.5 10.5 8 6 12.5" />
    </Icon>
  );
}

/// 存档中此箭头以 12px 面世(chevD),故默认尺寸异于家族其余的 15。
export function IcChevronDown({ size = 12 }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M3.5 6 8 10.5 12.5 6" />
    </Icon>
  );
}

/* ---- 操作 -------------------------------------------------------- */

export function IcCut({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <circle cx="4" cy="12" r="1.8" />
      <circle cx="12" cy="12" r="1.8" />
      <path d="M5.4 10.6 11 2.5M10.6 10.6 5 2.5" />
    </Icon>
  );
}

export function IcCopy({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.2" />
      <path d="M10.5 3.5h-7a1 1 0 0 0-1 1v7" />
    </Icon>
  );
}

export function IcPaste({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M6.5 3.5H4.2a1 1 0 0 0-1 1v8.3a1 1 0 0 0 1 1h7.6a1 1 0 0 0 1-1V4.5a1 1 0 0 0-1-1H9.5" />
      <path d="M6.5 2h3v2.6h-3z" />
    </Icon>
  );
}

export function IcDelete({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.7 8.2a1 1 0 0 0 1 .8h3.6a1 1 0 0 0 1-.8l.7-8.2" />
    </Icon>
  );
}

/* ---- 菜单 -------------------------------------------------------- */

export function IcSort({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M4 6.2h8M4 9.4h5.5M4 12.6h3" />
    </Icon>
  );
}

export function IcView({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="2.8" y="2.8" width="4.4" height="4.4" rx="1" />
      <rect x="8.8" y="2.8" width="4.4" height="4.4" rx="1" />
      <rect x="2.8" y="8.8" width="4.4" height="4.4" rx="1" />
      <rect x="8.8" y="8.8" width="4.4" height="4.4" rx="1" />
    </Icon>
  );
}

/* ---- 主题 -------------------------------------------------------- */

/// 月亮 + 三点(存档 v2 theme):三点实心 currentColor,覆盖根 fill=none。
export function IcTheme({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M8 2.2a5.8 5.8 0 1 0 0 11.6 5.8 5.8 0 0 1 0-11.6z" />
      <circle cx="11.6" cy="5" r=".7" fill="currentColor" />
      <circle cx="12.8" cy="8" r=".7" fill="currentColor" />
      <circle cx="11.2" cy="11" r=".7" fill="currentColor" />
    </Icon>
  );
}

/* ---- 地址 -------------------------------------------------------- */

export function IcCopyPath({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.2" />
      <path d="M10.5 3.5h-7a1 1 0 0 0-1 1v7" />
    </Icon>
  );
}

export function IcSearch({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <circle cx="7" cy="7" r="4.2" />
      <path d="m10.2 10.2 3 3" />
    </Icon>
  );
}

/* ---- 平台 -------------------------------------------------------- */

export function IcPc({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="2" y="3" width="12" height="8" rx="1.2" />
      <path d="M6 13.5h4M8 11v2.5" />
    </Icon>
  );
}

export function IcPhone({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <rect x="5" y="1.8" width="6" height="12.4" rx="1.4" />
      <path d="M7.2 12h1.6" />
    </Icon>
  );
}

/* ---- 抽屉操作 ---------------------------------------------------- */

export function IcPause({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M6 4v8M10 4v8" />
    </Icon>
  );
}

export function IcX({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M4.5 4.5l7 7M11.5 4.5l-7 7" />
    </Icon>
  );
}

export function IcRedo({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M13 6.5V3M13 6.5H9.5M13 6.5A5.2 5.2 0 1 0 12 11.8" />
    </Icon>
  );
}

export function IcTrash({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M3 4.5h10M6.5 4.5V3h3v1.5M4.5 4.5l.7 8.2a1 1 0 0 0 1 .8h3.6a1 1 0 0 0 1-.8l.7-8.2" />
    </Icon>
  );
}

/* ---- 杂项 -------------------------------------------------------- */

export function IcCheck({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
    </Icon>
  );
}

/* ---- 着色 -------------------------------------------------------- */

/// tintKey → 角标底色变量表;generic 无专属变量,取页缘色(见 IconTintKey 注)。
const TINT_FILL: Record<IconTintKey, string> = {
  pdf: "var(--icon-tint-pdf)",
  xls: "var(--icon-tint-xls)",
  vid: "var(--icon-tint-vid)",
  exe: "var(--icon-tint-exe)",
  txt: "var(--icon-tint-txt)",
  img: "var(--icon-tint-img)",
  archive: "var(--icon-tint-archive)",
  generic: "var(--icon-file-edge)",
};

/// 文件页形(存档 v2 filetint 去角标):页身/页缘/折角全引 body/edge 变量。
function FilePage() {
  return (
    <>
      <path d="M3.4 1.6h5.8l3.4 3.4v9.4H3.4z" fill="var(--icon-file-body)" stroke="var(--icon-file-edge)" />
      <path d="M9.2 1.6V5h3.4" fill="var(--icon-file-body)" stroke="var(--icon-file-edge)" />
    </>
  );
}

/// 文件夹(存档 v2 folder():原字面 #A8842F/#6E5620 逐位换 theme 变量)。
export function IcFolder({ size }: { size?: number }) {
  return (
    <TintIcon size={size}>
      <path
        d="M1.6 4.3c0-.6.5-1.1 1.1-1.1h3l1.5 1.7h6.2c.6 0 1.1.5 1.1 1.1v6.6c0 .6-.5 1.1-1.1 1.1H2.7c-.6 0-1.1-.5-1.1-1.1z"
        fill="var(--icon-folder-fill)"
        stroke="var(--icon-folder-edge)"
      />
    </TintIcon>
  );
}

/// 通用文件页形(无类型角标)。
export function IcFile({ size }: { size?: number }) {
  return (
    <TintIcon size={size}>
      <FilePage />
    </TintIcon>
  );
}

/// 类型角标文件页形:页形 + 圆角角标,底色按 tintKey 落 --icon-tint-*。
export function IcFileTyped({ tintKey, size }: { tintKey: IconTintKey; size?: number }) {
  return (
    <TintIcon size={size}>
      <FilePage />
      <rect x="5.2" y="8.2" width="5.6" height="3.4" rx=".8" fill={TINT_FILL[tintKey]} />
    </TintIcon>
  );
}

/* ---- 方向(托盘语义,存档 v4/v6 DIR) ----------------------------- */

export function IcDirRecv({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M2.5 10.5v1.6A1.4 1.4 0 0 0 3.9 13.5h8.2a1.4 1.4 0 0 0 1.4-1.4v-1.6" />
      <path d="M8 2.3v8.2M4.9 7.4 8 10.5l3.1-3.1" />
    </Icon>
  );
}

export function IcDirSend({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M2.5 10.5v1.6A1.4 1.4 0 0 0 3.9 13.5h8.2a1.4 1.4 0 0 0 1.4-1.4v-1.6" />
      <path d="M8 10.5V2.3M4.9 5.4 8 2.3l3.1 3.1" />
    </Icon>
  );
}

export function IcDirSwap({ size }: { size?: number }) {
  return (
    <Icon size={size}>
      <path d="M3 5h9M9.4 2.6 12 5.2 9.4 7.8" />
      <path d="M13 11H4M6.6 8.6 4 11.2l2.6 2.6" />
    </Icon>
  );
}
