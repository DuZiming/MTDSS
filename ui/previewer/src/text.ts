/// 文本/md 查看器(共骨架,spec §5 渲染器表「头部 64 KB 渐进,大文件续读」):
///
/// - md:markdown-it(html 透传)+ DOMPurify 消毒——XSS 面收敛于消毒管线(innerHTML 唯一
///   入口皆经 sanitize);代码/txt:扩展名猜语言 → highlight.js 精确高亮;猜不出=纯文本
///   textContent(零 HTML 解析面);
/// - 数据面:头部 TEXT_HEAD_BYTES(64KB)渐进首显;「加载更多」续读 TEXT_CHUNK_BYTES
///   (raw 带 Range,received 断点续读;Content-Range 解析总长,读尽即拆钮);
/// - 失败面:头部/续读失败=覆盖层+重试钮+retry_needed(重试=断点续读,幂等);
///   raw 缺席=终态 error;
/// - destroy:在途 abort、监听摘净、迟火按代次弃。
///
/// 依赖面(markdown-it/highlight.js/dompurify)按需动态 import:md 才装 markdown-it,
/// 有语言命中才装对应 hljs 语言模块;全走文案缝 strings。

import type { LanguageFn } from "highlight.js";
import type { PreviewStrings } from "./strings";
import { parseRangeTotal } from "./range";
import type { PreviewEvent, PreviewHandle, PreviewRequest } from "./types";

/// 头部渐进窗:64KB(spec「文本 64 KB」分段参数,常量收口便于调)。
export const TEXT_HEAD_BYTES = 64 * 1024;
/// 「加载更多」每窗:64KB(spec「文本 64 KB」)。
export const TEXT_CHUNK_BYTES = 64 * 1024;

/// 扩展名 → highlight.js 语言名(只列高亮价值高的常见面;猜不出=纯文本回退)。
const LANG_BY_EXT: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  tsx: "typescript",
  mts: "typescript",
  cts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  jsx: "javascript",
  py: "python",
  pyw: "python",
  go: "go",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  cs: "csharp",
  rb: "ruby",
  php: "php",
  sh: "shell",
  bash: "shell",
  zsh: "shell",
  ps1: "powershell",
  json: "json",
  jsonc: "json",
  yml: "yaml",
  yaml: "yaml",
  toml: "ini",
  ini: "ini",
  conf: "ini",
  sql: "sql",
  xml: "xml",
  html: "xml",
  htm: "xml",
  svg: "xml",
  css: "css",
  scss: "scss",
  less: "less",
  lua: "lua",
  diff: "diff",
  patch: "diff",
  mk: "makefile",
};

/// 语言模块装载门(静态可析动态 import——vite 逐枚分块,lib 单文件构建内联)。
const LANG_LOADERS: Record<string, () => Promise<{ default: LanguageFn }>> = {
  rust: () => import("highlight.js/lib/languages/rust"),
  typescript: () => import("highlight.js/lib/languages/typescript"),
  javascript: () => import("highlight.js/lib/languages/javascript"),
  python: () => import("highlight.js/lib/languages/python"),
  go: () => import("highlight.js/lib/languages/go"),
  java: () => import("highlight.js/lib/languages/java"),
  kotlin: () => import("highlight.js/lib/languages/kotlin"),
  swift: () => import("highlight.js/lib/languages/swift"),
  c: () => import("highlight.js/lib/languages/c"),
  cpp: () => import("highlight.js/lib/languages/cpp"),
  csharp: () => import("highlight.js/lib/languages/csharp"),
  ruby: () => import("highlight.js/lib/languages/ruby"),
  php: () => import("highlight.js/lib/languages/php"),
  shell: () => import("highlight.js/lib/languages/shell"),
  powershell: () => import("highlight.js/lib/languages/powershell"),
  json: () => import("highlight.js/lib/languages/json"),
  yaml: () => import("highlight.js/lib/languages/yaml"),
  ini: () => import("highlight.js/lib/languages/ini"),
  sql: () => import("highlight.js/lib/languages/sql"),
  xml: () => import("highlight.js/lib/languages/xml"),
  css: () => import("highlight.js/lib/languages/css"),
  scss: () => import("highlight.js/lib/languages/scss"),
  less: () => import("highlight.js/lib/languages/less"),
  lua: () => import("highlight.js/lib/languages/lua"),
  diff: () => import("highlight.js/lib/languages/diff"),
  makefile: () => import("highlight.js/lib/languages/makefile"),
};

/// highlight.js core 单飞:首次命中语言才装载;语言模块逐枚注册(包体考虑)。
/// 子路径无随包 d.ts,环境声明见 vendor.d.ts;API 形取根包自带类型。
type HljsApi = import("highlight.js").HLJSApi;
let hljsPromise: Promise<HljsApi> | null = null;
const registeredLangs = new Set<string>();

async function ensureHljs(lang: string): Promise<HljsApi> {
  if (hljsPromise === null) {
    hljsPromise = import("highlight.js/lib/core").then((m) => m.default);
  }
  const hljs = await hljsPromise;
  if (!registeredLangs.has(lang)) {
    const loader = LANG_LOADERS[lang];
    if (loader === undefined) throw new Error(`无语言装载门:${lang}`);
    hljs.registerLanguage(lang, (await loader()).default);
    registeredLangs.add(lang);
  }
  return hljs;
}

/// DOMPurify 消毒门单飞(innerHTML 唯一放行口;md 与代码高亮输出同管线)。
let sanitize: ((html: string) => string) | null = null;
let sanitizePromise: Promise<void> | null = null;

export async function ensureSanitize(): Promise<void> {
  if (sanitize !== null) return;
  if (sanitizePromise === null) {
    sanitizePromise = import("dompurify").then(({ default: DOMPurify }) => {
      // 外链防逃逸(评审 I3):预览内导航不改容器地址——A[href] 一律新开+noopener;
      // style 属性剔除(注入容器文档面而非 shadow,可反噬宿主 UI)
      DOMPurify.addHook("afterSanitizeAttributes", (node: Element) => {
        if (node.tagName === "A" && node.hasAttribute("href")) {
          node.setAttribute("target", "_blank");
          node.setAttribute("rel", "noopener noreferrer");
        }
      });
      sanitize = (html: string): string =>
        DOMPurify.sanitize(html, { FORBID_ATTR: ["style"] });
    });
  }
  await sanitizePromise;
}

/// markdown-it 渲染门单飞(仅 md 命中时装载;消毒走 ensureSanitize 同门)。
let mdRender: ((src: string) => string) | null = null;
let mdPromise: Promise<void> | null = null;

async function ensureMarkdown(): Promise<void> {
  if (mdRender !== null) return;
  if (mdPromise === null) {
    mdPromise = Promise.all([ensureSanitize(), import("markdown-it")]).then(
      ([, { default: MarkdownIt }]) => {
        // html:true=原始 HTML 过引擎透传,XSS 由 DOMPurify 消毒管线收口
        const md = new MarkdownIt({ html: true });
        mdRender = (src: string): string => md.render(src);
      },
    );
  }
  await mdPromise;
}

/// md 渲染公开门(field r16 #2:面板 md 封面复用同管线——markdown-it +
/// DOMPurify 消毒;冷装载后同步渲染)。
export async function renderMarkdownHtml(src: string): Promise<string> {
  await ensureMarkdown();
  return sanitize!(mdRender!(src));
}

/// 扩展名猜语言(meta.name;无扩展名/点首隐藏文件/未知扩展 → null=纯文本回退)。
function guessLanguage(name: string): string | null {
  const i = name.lastIndexOf(".");
  if (i <= 0) return null;
  return LANG_BY_EXT[name.slice(i + 1).toLowerCase()] ?? null;
}

type ListenerCleanup = {
  target: EventTarget;
  type: string;
  fn: EventListener;
};

export function mountText(
  el: HTMLElement,
  req: PreviewRequest,
  onEvent: (e: PreviewEvent) => void,
  strings: PreviewStrings,
): PreviewHandle {
  const url = req.urls.raw;
  const isMd = req.kind === "md";
  const lang = isMd ? null : guessLanguage(req.meta.name);

  let destroyed = false;
  /// 取数代次:重试/destroy 皆翻代,迟火按代弃。
  let gen = 0;
  /// 在飞门:续读钮双击/连点恰一窗(pdf.ts 同款)。
  let inFlight = false;
  let abortCtl: AbortController | null = null;
  let chunks: Uint8Array[] = [];
  let received = 0;
  let total: number | null = null;
  let done = false;

  // ---- 装配:内容行(toc? + 滚动面[md 正文 | pre>code] + 进度轨 + 搜索条)
  // ---- + 续读钮 + 失败覆盖层(field r15 #2:三件套) ----
  const root = document.createElement("div");
  root.className = "mtdss-preview mtdss-preview--text";

  /// 内容行:左 toc(md 有结构时)+ 右主区(相对定位承轨/搜索条)。
  const row = document.createElement("div");
  row.className = "mtdss-preview__text-row";

  const toc = document.createElement("nav");
  toc.className = "mtdss-preview__toc mtdss-preview__toc--hidden";
  toc.setAttribute("aria-label", "目录");
  row.appendChild(toc);

  const main = document.createElement("div");
  main.className = "mtdss-preview__text-main";

  const scroller = document.createElement("div");
  scroller.className = "mtdss-preview__text-scroll";
  let body: HTMLElement; // 渲染写入面(md 正文 div | code)
  if (isMd) {
    body = document.createElement("div");
    body.className = "mtdss-preview__md";
    scroller.appendChild(body);
  } else {
    const pre = document.createElement("pre");
    pre.className = "mtdss-preview__code";
    body = document.createElement("code");
    body.className = "mtdss-preview__code-code";
    if (lang !== null) body.classList.add(`language-${lang}`);
    pre.appendChild(body);
    scroller.appendChild(pre);
  }
  main.appendChild(scroller);

  // ---- 文本 minimap 宿主(r17 #3;替 r15 进度轨元素) ----
  const tmap = document.createElement("div");
  tmap.className = "mtdss-preview__tmap mtdss-preview__tmap--hidden";
  const tmapInner = document.createElement("div");
  tmapInner.className = "mtdss-preview__tmap-inner";
  const tmapViewport = document.createElement("div");
  tmapViewport.className = "mtdss-preview__tmap-viewport";
  tmap.append(tmapInner, tmapViewport);
  main.appendChild(tmap);

  // ---- 文档内搜索(r15 #2 Ctrl+F:高亮+上一个/下一个) ----
  const search = document.createElement("div");
  search.className = "mtdss-preview__search mtdss-preview__search--hidden";
  const searchInput = document.createElement("input");
  searchInput.type = "search";
  searchInput.className = "mtdss-preview__search-input";
  searchInput.placeholder = "文档内搜索…";
  searchInput.setAttribute("aria-label", "文档内搜索");
  const searchCount = document.createElement("span");
  searchCount.className = "mtdss-preview__search-count";
  searchCount.textContent = "";
  const searchPrev = document.createElement("button");
  searchPrev.type = "button";
  searchPrev.className = "mtdss-preview__search-btn";
  searchPrev.textContent = "↑";
  searchPrev.title = "上一个 (Shift+Enter)";
  const searchNext = document.createElement("button");
  searchNext.type = "button";
  searchNext.className = "mtdss-preview__search-btn";
  searchNext.textContent = "↓";
  searchNext.title = "下一个 (Enter)";
  const searchClose = document.createElement("button");
  searchClose.type = "button";
  searchClose.className = "mtdss-preview__search-btn";
  searchClose.textContent = "✕";
  searchClose.title = "关闭 (Esc)";
  search.append(searchInput, searchCount, searchPrev, searchNext, searchClose);
  main.appendChild(search);

  row.appendChild(main);
  root.appendChild(row);

  const bar = document.createElement("div");
  bar.className = "mtdss-preview__bar";
  const moreButton = document.createElement("button");
  moreButton.type = "button";
  moreButton.className = "mtdss-preview__button mtdss-preview__text-more";
  moreButton.textContent = strings.textLoadMore;
  moreButton.hidden = true;
  bar.appendChild(moreButton);
  root.appendChild(bar);

  const overlay = document.createElement("div");
  overlay.className = "mtdss-preview__overlay mtdss-preview__overlay--hidden";
  const note = document.createElement("div");
  note.className = "mtdss-preview__note";
  note.textContent = strings.textLoadFailed;
  const retryButton = document.createElement("button");
  retryButton.type = "button";
  retryButton.className = "mtdss-preview__button";
  retryButton.textContent = strings.retry;
  overlay.append(note, retryButton);
  root.appendChild(overlay);
  el.appendChild(root);

  // ---- 监听记账:destroy 一次摘净 ----
  const cleanups: ListenerCleanup[] = [];
  const listen = (target: EventTarget, type: string, fn: EventListener): void => {
    target.addEventListener(type, fn);
    cleanups.push({ target, type, fn });
  };

  // ---- 数据面:64KB 窗序列(Range 断点续读) ----
  const decodeSoFar = (): string => {
    const all = new Uint8Array(received);
    let off = 0;
    for (const c of chunks) {
      all.set(c, off);
      off += c.length;
    }
    return new TextDecoder().decode(all);
  };

  const render = async (): Promise<void> => {
    const text = decodeSoFar();
    if (isMd) {
      await ensureMarkdown();
      body.innerHTML = sanitize!(mdRender!(text));
    } else if (lang !== null) {
      await ensureSanitize(); // 消毒门随首挂初始化(冷模块代码首挂无此步=踩 null 永败)
      const hljs = await ensureHljs(lang);
      body.innerHTML = sanitize!(hljs.highlight(text, { language: lang }).value);
    } else {
      body.textContent = text; // 纯文本面:零 HTML 解析,天然无注入
    }
    rebuildToc(); // 结构目录随渲染重建(续读窗口后同刷;md=DOM 锚,txt=行估算)
    updateTextMap(); // 文本 minimap:克隆缩放随渲染翻新
  };

  // ---- 文本 minimap(field r17 #3:VS Code 式——整档缩微渲染+视口浮层,
  // 点击/拖拽跳转;替 r15 细轨) ----
  const updateTextMap = (): void => {
    const scrollH0 = scroller.scrollHeight;
    const clientH0 = scroller.clientHeight;
    // 判定面只看滚动语义(先显后量:tmap 以 --hidden 起步,display:none 下
    // clientHeight=0,先量后显=自锁死)。
    if (scrollH0 <= clientH0 + 4) {
      tmap.classList.add("mtdss-preview__tmap--hidden");
      return;
    }
    tmap.classList.remove("mtdss-preview__tmap--hidden");
    const contentEl = isMd ? body : (body.parentElement as HTMLElement); // md=正文 | 纯码=pre
    const contentW = isMd ? scroller.clientWidth : contentEl.scrollWidth || contentEl.clientWidth;
    const contentH = contentEl.scrollHeight;
    const viewH = tmap.clientHeight;
    const mapW = tmap.clientWidth;
    if (contentW <= 0 || contentH <= 0 || viewH <= 0 || mapW <= 0) return;
    const k = mapW / contentW; // 按宽缩放(行宽∝行长,VS Code 同构)
    if (tmapInner.dataset.k !== k.toFixed(4) || tmapInner.childElementCount === 0) {
      tmapInner.dataset.k = k.toFixed(4);
      tmapInner.textContent = "";
      const clone = contentEl.cloneNode(true) as HTMLElement;
      clone.removeAttribute("class");
      clone.style.margin = "0";
      clone.style.whiteSpace = "pre"; // 缩微面不折行(行宽∝行长,VS Code 同构)
      tmapInner.appendChild(clone);
    }
    tmapInner.style.transform = `scale(${k})`;
    const mapH = contentH * k;
    const scrollH = scroller.scrollHeight;
    const clientH = scroller.clientHeight;
    const maxShift = Math.max(0, mapH - viewH);
    const shift =
      scrollH > clientH ? maxShift * (scroller.scrollTop / (scrollH - clientH)) : 0;
    tmapInner.style.transform = `translateY(${-shift}px) scale(${k})`;
    const vpH = Math.min(viewH, Math.max(14, clientH * k));
    const vpTop = Math.min(viewH - vpH, Math.max(0, scroller.scrollTop * k - shift));
    tmapViewport.style.height = `${vpH}px`;
    tmapViewport.style.top = `${vpTop}px`;
  };
  listen(scroller, "scroll", () => {
    updateTextMap();
    trackActiveHeading();
  });
  let tmapDrag: { pointerId: number } | null = null;
  const tmapJump = (clientY: number): void => {
    const rect = tmap.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (clientY - rect.top) / rect.height));
    scroller.scrollTop = ratio * (scroller.scrollHeight - scroller.clientHeight);
  };
  listen(tmap, "pointerdown", (raw) => {
    const ev = raw as PointerEvent;
    if (ev.button !== 0) return;
    tmapDrag = { pointerId: ev.pointerId };
    tmapJump(ev.clientY);
  });
  listen(window, "pointermove", (raw) => {
    if (tmapDrag == null) return;
    const ev = raw as PointerEvent;
    if (ev.pointerId !== tmapDrag.pointerId) return;
    tmapJump(ev.clientY);
  });
  listen(window, "pointerup", (raw) => {
    const ev = raw as PointerEvent;
    if (tmapDrag != null && ev.pointerId === tmapDrag.pointerId) tmapDrag = null;
  });

  // ---- 目录(r15 #2 md;r17 #3 扩 txt 启发式):点击跳转,滚动跟随高亮 ----
  /// 重渲染(续读)后按序重建。md=h1-h3 DOM 锚(id 按序号稳定);txt=行
  /// 扫描启发式(#前缀/第N章节/Chapter N/N. 标题;≤200 行窗),跳转按
  /// 行高估算(pre 不折行,scrollHeight/行数=恒行高)。
  /// txt 启发式行 → {level, label, line}。
  const TXT_HEAD_RE =
    /^(?:(#{1,6})\s+(\S.{0,78})|(第[一二三四五六七八九十百千0-9]+[章节篇部回][^\n]{0,60})|([Cc][Hh][Aa][Pp][Tt][Ee][Rr]\s+\d+[^\n]{0,60})|(\d{1,3}[.、]\s*\S[^\n]{0,60}))$/;
  const txtOutline = (text: string): { level: number; label: string; line: number }[] => {
    const out: { level: number; label: string; line: number }[] = [];
    const lines = text.split(/\r?\n/);
    for (let i = 0; i < Math.min(lines.length, 200); i += 1) {
      const m = TXT_HEAD_RE.exec(lines[i]!.trim());
      if (m == null) continue;
      if (m[1] != null) out.push({ level: Math.min(3, m[1].length), label: m[2]!, line: i });
      else if (m[3] != null || m[4] != null) out.push({ level: 1, label: (m[3] ?? m[4])!, line: i });
      else out.push({ level: 2, label: m[5]!, line: i });
    }
    return out;
  };
  const rebuildToc = (): void => {
    toc.textContent = "";
    if (isMd) {
      const headings = Array.from(body.querySelectorAll("h1, h2, h3")) as HTMLElement[];
      if (headings.length < 2) {
        toc.classList.add("mtdss-preview__toc--hidden");
        return;
      }
      toc.classList.remove("mtdss-preview__toc--hidden");
      headings.forEach((h, i) => {
        if (h.id === "") h.id = `mtdss-toc-${i}`;
        const item = document.createElement("button");
        item.type = "button";
        item.className = `mtdss-preview__toc-item mtdss-preview__toc-item--${h.tagName.toLowerCase()}`;
        item.textContent = (h.textContent ?? "").trim() || "(无标题)";
        item.title = item.textContent;
        listen(item, "click", () => {
          h.scrollIntoView({ behavior: "smooth", block: "start" });
        });
        toc.appendChild(item);
      });
      return;
    }
    // txt/代码:启发式大纲(纯文本面才扫;代码高亮面标题噪声大,同样适用)
    const text = decodeSoFar();
    const outline = txtOutline(text);
    if (outline.length < 2) {
      toc.classList.add("mtdss-preview__toc--hidden");
      return;
    }
    toc.classList.remove("mtdss-preview__toc--hidden");
    const totalLines = text.split(/\r?\n/).length || 1;
    for (const head of outline) {
      const item = document.createElement("button");
      item.type = "button";
      item.className = `mtdss-preview__toc-item mtdss-preview__toc-item--h${Math.min(3, head.level + 1)}`;
      item.textContent = head.label;
      item.title = head.label;
      listen(item, "click", () => {
        const pre = body.parentElement as HTMLElement;
        const lineH = pre.scrollHeight / Math.max(totalLines, 1);
        scroller.scrollTo({ top: Math.max(0, head.line * lineH - 8), behavior: "smooth" });
      });
      toc.appendChild(item);
    }
  };
  const trackActiveHeading = (): void => {
    if (toc.classList.contains("mtdss-preview__toc--hidden")) return;
    const items = Array.from(toc.children) as HTMLElement[];
    const headings = Array.from(body.querySelectorAll("h1, h2, h3")) as HTMLElement[];
    if (items.length === 0 || headings.length !== items.length) return;
    const top = scroller.scrollTop + 32;
    let active = -1;
    for (let i = 0; i < headings.length; i += 1) {
      if (headings[i]!.offsetTop <= top) active = i;
    }
    items.forEach((item, i) => {
      item.classList.toggle("mtdss-preview__toc-item--active", i === active);
    });
  };

  // ---- 文档内搜索(r15 #2 Ctrl+F):Text Walker 高亮 + 上/下跳 ----
  let marks: HTMLElement[] = [];
  let activeMark = -1;
  const searchDebounce = { timer: null as ReturnType<typeof setTimeout> | null };
  const clearMarks = (): void => {
    for (const mark of marks) {
      const parent = mark.parentNode;
      if (parent == null) continue;
      parent.replaceChild(document.createTextNode(mark.textContent ?? ""), mark);
      parent.normalize();
    }
    marks = [];
    activeMark = -1;
    searchCount.textContent = "";
  };
  const applySearch = (query: string): void => {
    clearMarks();
    const q = query.trim();
    if (q.length === 0) return;
    const needle = q.toLowerCase();
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    const targets: Text[] = [];
    let node = walker.nextNode();
    while (node != null) {
      targets.push(node as Text);
      node = walker.nextNode();
    }
    for (const textNode of targets) {
      const hay = (textNode.nodeValue ?? "").toLowerCase();
      if (!hay.includes(needle)) continue;
      const frag = document.createDocumentFragment();
      const src = textNode.nodeValue ?? "";
      let i = 0;
      for (;;) {
        const hit = hay.indexOf(needle, i);
        if (hit < 0) break;
        if (hit > i) frag.appendChild(document.createTextNode(src.slice(i, hit)));
        const mark = document.createElement("mark");
        mark.className = "mtdss-preview__hit";
        mark.textContent = src.slice(hit, hit + q.length);
        frag.appendChild(mark);
        marks.push(mark);
        i = hit + q.length;
      }
      if (i < src.length) frag.appendChild(document.createTextNode(src.slice(i)));
      textNode.parentNode?.replaceChild(frag, textNode);
    }
    if (marks.length > 0) goMark(0);
    else searchCount.textContent = "0/0";
  };
  const goMark = (idx: number): void => {
    if (marks.length === 0) return;
    activeMark = ((idx % marks.length) + marks.length) % marks.length;
    marks.forEach((m, i) => m.classList.toggle("mtdss-preview__hit--active", i === activeMark));
    marks[activeMark]!.scrollIntoView({ block: "center" });
    searchCount.textContent = `${activeMark + 1}/${marks.length}`;
  };
  const openSearch = (): void => {
    search.classList.remove("mtdss-preview__search--hidden");
    searchInput.focus();
    searchInput.select();
  };
  const closeSearch = (): void => {
    search.classList.add("mtdss-preview__search--hidden");
    clearMarks();
    searchInput.value = "";
  };
  listen(document, "keydown", (raw) => {
    const ev = raw as KeyboardEvent;
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "f") {
      ev.preventDefault();
      openSearch();
    }
  });
  listen(searchInput, "input", () => {
    if (searchDebounce.timer != null) clearTimeout(searchDebounce.timer);
    searchDebounce.timer = setTimeout(() => {
      searchDebounce.timer = null;
      applySearch(searchInput.value);
    }, 200);
  });
  listen(searchInput, "keydown", (raw) => {
    const ev = raw as KeyboardEvent;
    if (ev.key === "Enter") {
      ev.preventDefault();
      goMark(activeMark + (ev.shiftKey ? -1 : 1));
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      closeSearch();
      scroller.focus();
    }
  });
  listen(searchPrev, "click", () => goMark(activeMark - 1));
  listen(searchNext, "click", () => goMark(activeMark + 1));
  listen(searchClose, "click", closeSearch);

  const updateBar = (): void => {
    if (done) {
      moreButton.remove();
      bar.hidden = true; // 读完:续读栏整体隐没
    } else {
      moreButton.hidden = false;
    }
  };

  const fetchWindow = async (): Promise<void> => {
    const myGen = ++gen;
    const begin = received;
    const windowBytes = chunks.length === 0 ? TEXT_HEAD_BYTES : TEXT_CHUNK_BYTES;
    const controller = (abortCtl = new AbortController());
    const res = await fetch(url!, {
      headers: { Range: `bytes=${begin}-${begin + windowBytes - 1}` },
      signal: controller.signal,
    });
    if (destroyed || gen !== myGen) return;
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const data = new Uint8Array(await res.arrayBuffer());
    if (destroyed || gen !== myGen) return;
    const headerTotal = parseRangeTotal(res.headers?.get("content-range") ?? null);
    if (headerTotal !== null) {
      total = headerTotal;
    } else if (begin === 0) {
      total = data.length; // 无 content-range=整取即全量
    }
    chunks.push(data);
    received += data.length;
    done = total !== null && received >= total;
    await render();
    if (destroyed || gen !== myGen) return;
    if (chunks.length === 1) {
      // meta=主内容就绪面:头部窗一发,续窗不重发
      onEvent({ type: "meta", bytes: total ?? received });
    }
    updateBar();
  };

  const runFetch = async (): Promise<void> => {
    if (destroyed || inFlight) return; // 在飞门:双击/连点恰一窗(同 received 起点双取=重复块+断点错位)
    inFlight = true;
    try {
      await fetchWindow();
    } catch {
      if (destroyed) return;
      overlay.classList.remove("mtdss-preview__overlay--hidden");
      onEvent({ type: "retry_needed", message: strings.textLoadFailed });
    } finally {
      inFlight = false;
    }
  };

  listen(moreButton, "click", () => {
    void runFetch();
  });
  listen(retryButton, "click", () => {
    if (destroyed) return;
    overlay.classList.add("mtdss-preview__overlay--hidden");
    void runFetch(); // 断点续读:received 不回退,幂等
  });

  // ---- boot ----
  if (url === undefined) {
    // 装配即不可预览:终态 error,无重试面
    retryButton.remove();
    overlay.classList.remove("mtdss-preview__overlay--hidden");
    onEvent({ type: "error", message: strings.unavailableMessage });
  } else {
    void runFetch();
  }

  // ---- destroy:监听摘净 + 在途 abort + 迟火弃 ----
  return {
    destroy: () => {
      if (destroyed) return;
      destroyed = true;
      gen += 1;
      abortCtl?.abort();
      for (const entry of [...cleanups]) {
        entry.target.removeEventListener(entry.type, entry.fn);
      }
      root.remove();
    },
  };
}
