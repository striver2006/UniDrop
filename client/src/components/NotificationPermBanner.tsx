import React from "react";
import { BellOff, ExternalLink } from "lucide-react";

interface Props {
  /** null = 本平台无此机制或尚未探测，不显示横幅。 */
  granted: boolean | null;
  onOpenSettings: () => void;
}

/**
 * 系统通知授权被拒时的引导横幅。
 *
 * 为什么需要它：授权失效（重装后记录翻转、系统设置被改）时应用侧**没有任何
 * 错误**——传输照常完成、剪贴板照常写入，只是通知被系统静默丢弃。菜单栏
 * 应用平时窗口是隐藏的，应用内 toast 也看不见，用户视角就是「收到了但
 * 没人告诉我」。2026-09-14 的实机日志里它以 UNErrorDomain error 1 的形式
 * 整整潜伏了一个下午。这条横幅的全部价值就是把那个哑故障翻译成一句人话
 * 加一条可执行路径。
 *
 * 配色刻意与 MenuBarHiddenBanner 同用琥珀：应用本身完全正常，只是少了
 * 一条提示通道；用 connError 的玫红会让用户以为传输坏了。
 *
 * 「暂不」只隐藏本次会话，**刻意不持久化**：通知是收件提示的唯一通道，
 * 每次启动仍然被拒就该再出现一次，直到用户真正处理。
 */
export const NotificationPermBanner: React.FC<Props> = ({ granted, onOpenSettings }) => {
  const [dismissed, setDismissed] = React.useState(false);

  if (granted !== false || dismissed) return null;

  return (
    <div className="bg-amber-950/80 border-b border-amber-800/80 px-4 py-2 text-xs text-amber-200">
      <div className="flex items-start justify-between gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <BellOff className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />
          <div className="min-w-0 space-y-1">
            <p className="font-medium break-words">系统通知未开启</p>
            <p className="break-words text-amber-200/80">
              其他设备发来的文本、图片、文件仍会正常接收并写入剪贴板/沙盒，
              但不会弹出任何提示。菜单栏应用平时窗口是隐藏的，收件时若不在电脑前，
              很容易错过内容。
            </p>
            <p className="break-words text-amber-200/80">
              打开 <span className="text-amber-100">系统设置 → 通知</span>，
              找到 <span className="text-amber-100">瞬贴 (UniDrop)</span> 并开启
              <span className="text-amber-100">「允许通知」</span>。开启后无需重启，
              下一条通知即会弹出。
            </p>
          </div>
        </div>

        <div className="flex flex-col gap-1 shrink-0">
          <button
            onClick={onOpenSettings}
            className="flex items-center gap-1 px-2 py-1 rounded bg-amber-800/60 hover:bg-amber-700/60 text-amber-100 transition-colors whitespace-nowrap"
          >
            <ExternalLink className="w-3 h-3" />
            打开系统设置
          </button>
          <button
            onClick={() => setDismissed(true)}
            className="px-2 py-1 rounded bg-amber-900/40 hover:bg-amber-800/40 text-amber-200/80 transition-colors whitespace-nowrap"
          >
            暂不
          </button>
        </div>
      </div>
    </div>
  );
};
