/// 平铺尺寸档(V4-1 T19):三档小/中/大,档位由 App 会话态持有(与
/// sortState 同款「仅内存态」先例;brief 只钉三档 CSS 变量,档位不持久),
/// FileTiles 以 data-tile-size 落档,App.css 三档 --tile-* 变量承载实际
/// 尺寸(列宽/槽内图标,78/66 存档面=m 档)。Ctrl+滚轮翻档走本模块
/// 纯函数(方向:滚轮上=放大,滚轮下=缩小;端点钳制)。

/// 三档位。
export type TileSize = "s" | "m" | "l";

/// 档序(小→大;钳制与测试锚)。
export const TILE_SIZES: readonly TileSize[] = ["s", "m", "l"];

/// 每档缩略图请求宽(回环口 /thumb?w=;daemon 侧钳上界,s/m/l 递清)。
export const TILE_THUMB_W: Record<TileSize, number> = { s: 128, m: 256, l: 384 };

/// 翻档:deltaY<0(滚轮上)放大一档,>0 缩小一档,0 不动;端点钳制。
export function stepTileSize(cur: TileSize, deltaY: number): TileSize {
  const i = TILE_SIZES.indexOf(cur);
  const next = i + (deltaY < 0 ? 1 : deltaY > 0 ? -1 : 0);
  return TILE_SIZES[Math.max(0, Math.min(TILE_SIZES.length - 1, next))];
}

/// 每档类型图标渲染像素(field r19 #1:图标按真实显示尺寸渲染,不再
/// 16px 属性被 CSS 放大数倍——笔画亚像素走样=「平铺放大后模糊」;与
/// App.css --tile-icon-* 三档变量同值,改档同步两处)。
export const TILE_ICON_DIR: Record<TileSize, number> = { s: 54, m: 78, l: 104 };
export const TILE_ICON_FILE: Record<TileSize, number> = { s: 46, m: 66, l: 88 };
