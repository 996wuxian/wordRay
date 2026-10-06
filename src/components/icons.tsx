/**
 * 应用图标 —— Tabler Icons（MIT）
 *
 * 沿用 doTime 项目的封装方式：业务代码**只引用这里的语义化名字**，
 * 不直接 import 图标库。这样换图标库、调统一尺寸/线宽都只改这一个文件。
 *
 * @see https://tabler.io/icons
 */

import "./icons.css";
import {
  IconArrowBackUp as TbArrowBackUp,
  IconCheck as TbCheck,
  IconClipboardText as TbClipboardText,
  IconDeviceFloppy as TbDeviceFloppy,
  IconEye as TbEye,
  IconEyeOff as TbEyeOff,
  IconHistory as TbHistory,
  IconLayoutColumns as TbLayoutColumns,
  IconLayoutRows as TbLayoutRows,
  IconLoader2 as TbLoader2,
  IconPin as TbPin,
  IconPinnedOff as TbPinnedOff,
  IconPlugConnected as TbPlugConnected,
  IconSettings as TbSettings,
  IconTrash as TbTrash,
  IconX as TbX,
} from "@tabler/icons-react";

export type AppIconProps = {
  size?: number;
  title?: string;
  className?: string;
  stroke?: number;
};

/**
 * 图标组件的类型直接从实际组件上取。
 *
 * 不手写 `ComponentType<{...}>`：Tabler 的 `stroke` 是 `string | number`，
 * 手写的窄类型会在 `propTypes` 上与它不兼容而报 TS2345。
 * 包也没有导出 `TablerIcon` 类型别名，所以用 `typeof` 拿最准。
 */
type TablerIcon = typeof TbPin;

/**
 * 统一包装：固定尺寸、线宽、"跟随文字颜色"。
 *
 * `title` 给了就暴露给无障碍树，没给就当作纯装饰（`aria-hidden`）——
 * 按钮本身另有 `title`，图标不必重复播报。
 */
function wrap(Tb: TablerIcon, defaults?: { stroke?: number; className?: string }) {
  function AppIcon({
    size = 16,
    title,
    stroke = defaults?.stroke ?? 1.75,
    className = "",
  }: AppIconProps) {
    const classes = ["app-icon", defaults?.className, className].filter(Boolean).join(" ");
    return (
      <span
        className={classes}
        style={{
          display: "inline-flex",
          width: size,
          height: size,
          lineHeight: 0,
          color: "currentColor",
          flexShrink: 0,
          alignItems: "center",
          justifyContent: "center",
        }}
        role={title ? "img" : undefined}
        aria-hidden={title ? undefined : true}
        aria-label={title}
        title={title}
      >
        <Tb size={size} stroke={stroke} color="currentColor" aria-hidden />
      </span>
    );
  }
  return AppIcon;
}

// 翻译面板
export const IconCopy = wrap(TbClipboardText);
export const IconCopied = wrap(TbCheck);
export const IconSettings = wrap(TbSettings);
export const IconHistory = wrap(TbHistory);
export const IconPin = wrap(TbPin);
export const IconPinOff = wrap(TbPinnedOff);
export const IconClose = wrap(TbX);
export const IconLayoutColumns = wrap(TbLayoutColumns);
export const IconLayoutRows = wrap(TbLayoutRows);

// 设置窗口
export const IconSave = wrap(TbDeviceFloppy);
export const IconTest = wrap(TbPlugConnected);
export const IconTrash = wrap(TbTrash);
export const IconReveal = wrap(TbEye);
export const IconHide = wrap(TbEyeOff);
export const IconReset = wrap(TbArrowBackUp);
export const IconBusy = wrap(TbLoader2, { className: "spin" });
