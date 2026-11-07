/// DeviceTable SSR 冒烟(返工裁定六 §18.3 平台图标映射;react-dom/server
/// 纯字符串渲染)。钉死:①ios/android → 手机形(§18.3:ios 曾落 PC 形
/// =bug);②windows/macos/未知 → PC 形兜底(零回归);③离线行灰显形态
/// 不受映射影响。SVG 几何钉法(icons.tsx 存档):手机形独有 path
/// 「M7.2 12h1.6」(机身下缘听标),PC 形独有 rect「x=2 y=3」(显示器身)。
/// 双击进入为交互,SSR 不可测,不在本文件覆盖。
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import "../i18n";

import type { DeviceInfo } from "../lib/derived";
import DeviceTable from "./DeviceTable";

const devOf = (id: string, name: string, platform: string, online = true): DeviceInfo => ({
  device_id: id,
  name,
  platform,
  online,
});

const noop = () => {};

const PHONE_GLYPH = 'd="M7.2 12h1.6"';
const PC_GLYPH = '<rect x="2" y="3"';

const renderTable = (devices: DeviceInfo[]) =>
  renderToStaticMarkup(
    createElement(DeviceTable, { devices, selfDeviceId: null, onOpenDevice: noop }),
  );

describe("DeviceTable 平台图标(§18.3:ios/android → 手机形)", () => {
  it("ios → 手机形(PC 形不在场)", () => {
    const html = renderTable([devOf("d1", "口袋机", "ios")]);
    expect(html).toContain(PHONE_GLYPH);
    expect(html).not.toContain(PC_GLYPH);
  });

  it("android → 手机形(既有裁定零回归)", () => {
    const html = renderTable([devOf("d1", "口袋机", "android")]);
    expect(html).toContain(PHONE_GLYPH);
    expect(html).not.toContain(PC_GLYPH);
  });

  it("windows/macos/未知 → PC 形兜底(手机形不在场;零回归)", () => {
    for (const platform of ["windows", "macos", "freebsd"]) {
      const html = renderTable([devOf("d1", "工位机", platform)]);
      expect(html).toContain(PC_GLYPH);
      expect(html).not.toContain(PHONE_GLYPH);
    }
  });

  it("混合平台各行各落其形;离线行灰显同款映射", () => {
    const html = renderTable([
      devOf("d1", "手机A", "ios", true),
      devOf("d2", "电脑B", "windows", false),
    ]);
    expect(html).toContain(PHONE_GLYPH);
    expect(html).toContain(PC_GLYPH);
    expect(html).toContain("offline");
  });
});
