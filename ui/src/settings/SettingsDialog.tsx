/// 设置对话框(V3-4 §5-0,Windows 设置面新建):工具栏齿轮入口的模态
/// 面板,四项 + 「限制本机被浏览范围」+ 检查更新入口 + 关于。
///
/// 数据面:设置快照由 App 持有(daemon settings.get 拉取,引擎
/// settings.json 为持久化权威),本组件纯受控渲染;全部变更即时应用——
/// 开关/信任目录/设备名统一走 onPatch(patch)回调,App 落 settings.set
/// RPC 后回流新快照(信任段变更由 App 另发 roots.set 热注册,本组件
/// 零感知)。检查更新复用既有更新页覆盖层(onOpenUpdate)。
///
/// 分组(spec §5-1..4):设备名(输入 + 保存,改名下次登录生效)/ 显示
/// 隐藏文件 / 允许删除远端文件 / 限制本机被浏览范围(开关 + 目录选择
/// 器列表,pick_folder 添加、逐行移除)/ 检查更新… / 关于(版本)。
/// Escape 与遮罩点击 = 关闭(即时应用语义,无未保存态);busy 在途
/// 禁全部控件防连点。样式沿 .dialog 面,新增 .settings-* 行族。
import { useEffect, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useTranslation } from "react-i18next";

import { pickFolder } from "../lib/daemon";
import { useFocusTrap } from "../lib/useFocusTrap";
import type { SettingsPatch, V34Settings } from "./model";

/// 单行开关(三扇共用):role="switch" + aria-checked,左词面+提示 /
/// 右开关。busy 在途禁点。
function SwitchRow({
  label,
  hint,
  checked,
  disabled,
  onToggle,
}: {
  label: string;
  hint?: string;
  checked: boolean;
  disabled: boolean;
  onToggle(next: boolean): void;
}) {
  return (
    <div className="settings-row">
      <div className="settings-row-text">
        <span className="settings-row-label">{label}</span>
        {hint != null && <span className="settings-row-hint">{hint}</span>}
      </div>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        className={"settings-switch" + (checked ? " on" : "")}
        disabled={disabled}
        onClick={() => onToggle(!checked)}
      >
        <span className="settings-switch-knob" aria-hidden="true" />
      </button>
    </div>
  );
}

export default function SettingsDialog({
  open,
  settings,
  version,
  busy,
  onPatch,
  onOpenUpdate,
  onClose,
}: {
  open: boolean;
  /// 权威设置快照(App 持有;patch 应用后回流)。
  settings: V34Settings;
  /// 关于段展示的应用版本(hello.version;未知 → "—")。
  version: string;
  /// 任一 patch RPC 在途(App 侧忙标):禁全部控件防连点/半程态。
  busy: boolean;
  onPatch(patch: SettingsPatch): void;
  onOpenUpdate(): void;
  onClose(): void;
}) {
  const { t } = useTranslation();
  const ref = useFocusTrap(open);
  /// 设备名输入(每次打开从快照重置;保存钮提交,不受 busy 外的即时
  /// patch 流影响)。
  const [nameDraft, setNameDraft] = useState(settings.device_name ?? "");
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    if (open) setNameDraft(settings.device_name ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  /// busy 在途取消不受理(防半程态,FavNameDialog 同款口径)。
  const cancel = () => {
    if (busy) return;
    onClose();
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === "Escape") cancel();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, busy]);

  if (!open) return null;

  /// 设备名保存:去首尾空白,空串 = 清空覆盖(device_name:null,登录
  /// 回落主机名);与快照同值时按钮禁用(无变化不发货)。
  const nameTrimmed = nameDraft.trim();
  const nameDirty = nameTrimmed !== (settings.device_name ?? "");

  /// 添加信任目录:系统目录选择器 → 追加进 trusted_roots(patch 整单
  /// 换入,App 落盘 + roots.set 热注册)。重复路径不重复入表。选择器
  /// 失败/取消静默保持原状(选目录是可选增强,pickCaCert 同款口径)。
  const addTrustDir = async () => {
    if (busy || picking) return;
    setPicking(true);
    try {
      const dir = await pickFolder();
      if (dir != null && !settings.trusted_roots.includes(dir)) {
        onPatch({ trusted_roots: [...settings.trusted_roots, dir] });
      }
    } catch {
      // 选择器不可用:保持原状
    } finally {
      setPicking(false);
    }
  };

  const removeTrustDir = (dir: string) => {
    if (busy) return;
    onPatch({ trusted_roots: settings.trusted_roots.filter((d) => d !== dir) });
  };

  return (
    <div className="dialog-mask" role="presentation" onClick={cancel}>
      <div
        ref={ref}
        className="dialog settings-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={t("settings.title")}
        onClick={(e) => e.stopPropagation()}
      >
        <p className="dialog-title">{t("settings.title")}</p>

        <div className="settings-body">
          {/* 设备名(§5-4):迁自登录高级选项;保存钮提交,改名下次登录生效 */}
          <div className="settings-row settings-row-stack">
            <span className="settings-row-label">{t("settings.deviceName")}</span>
            <div className="settings-name-line">
              <input
                className="settings-name-input"
                type="text"
                value={nameDraft}
                placeholder={t("settings.deviceNamePlaceholder")}
                aria-label={t("settings.deviceName")}
                onChange={(e) => setNameDraft(e.target.value)}
                onKeyDown={(e: ReactKeyboardEvent<HTMLInputElement>) => {
                  if (e.key === "Enter" && nameDirty && !busy) {
                    onPatch({ device_name: nameTrimmed === "" ? null : nameTrimmed });
                  }
                }}
              />
              <button
                type="button"
                className="btn"
                disabled={busy || !nameDirty}
                onClick={() =>
                  onPatch({ device_name: nameTrimmed === "" ? null : nameTrimmed })
                }
              >
                {t("settings.deviceNameSave")}
              </button>
            </div>
            <span className="settings-row-hint">{t("settings.deviceNameHint")}</span>
          </div>

          {/* 显示隐藏文件(§5-3):默认关 = 滤 dotfile */}
          <SwitchRow
            label={t("settings.showHidden")}
            hint={t("settings.showHiddenHint")}
            checked={settings.show_hidden}
            disabled={busy}
            onToggle={(v) => onPatch({ show_hidden: v })}
          />

          {/* 允许删除远端文件(§5-1):默认关 = 远端删除入口收门,回收站确认梯保留 */}
          <SwitchRow
            label={t("settings.allowRemoteDelete")}
            hint={t("settings.allowRemoteDeleteHint")}
            checked={settings.allow_remote_delete}
            disabled={busy}
            onToggle={(v) => onPatch({ allow_remote_delete: v })}
          />

          {/* 限制本机被浏览范围(§5-2):默认关 = 全盘直通;开 = 收件箱+所选目录,
              切换热生效(App 落盘后另发 roots.set,免重启) */}
          <SwitchRow
            label={t("settings.trustMode")}
            hint={t("settings.trustModeHint")}
            checked={settings.trust_mode}
            disabled={busy}
            onToggle={(v) => onPatch({ trust_mode: v })}
          />
          {settings.trust_mode && (
            <div className="settings-trust-list">
              {settings.trusted_roots.length === 0 && (
                <div className="settings-trust-empty">{t("settings.trustEmpty")}</div>
              )}
              {settings.trusted_roots.map((dir) => (
                <div key={dir} className="settings-trust-row">
                  <span className="settings-trust-path" title={dir}>
                    {dir}
                  </span>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    aria-label={`${t("settings.trustRemove")}: ${dir}`}
                    onClick={() => removeTrustDir(dir)}
                  >
                    {t("settings.trustRemove")}
                  </button>
                </div>
              ))}
              <button type="button" className="btn" disabled={busy || picking} onClick={() => void addTrustDir()}>
                {t("settings.trustAdd")}
              </button>
            </div>
          )}

          {/* 检查更新入口(复用既有更新页覆盖层;App 关设置面开更新页) */}
          <div className="settings-row">
            <span className="settings-row-label">{t("update.title")}</span>
            <button type="button" className="btn" disabled={busy} onClick={onOpenUpdate}>
              {t("settings.checkUpdate")}
            </button>
          </div>

          {/* 关于:应用名 + 版本(hello.version 透传) */}
          <div className="settings-row">
            <span className="settings-row-label">{t("settings.about")}</span>
            <span className="settings-version">
              {t("settings.version")} {version === "" ? "—" : version}
            </span>
          </div>
        </div>

        <div className="dialog-actions">
          <button className="btn" onClick={cancel} disabled={busy}>
            {t("settings.close")}
          </button>
        </div>
      </div>
    </div>
  );
}
