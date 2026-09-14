import React from "react";
import { MonitorX, ExternalLink } from "lucide-react";
import { TrayPlacementPayload } from "../types";

interface Props {
  payload: TrayPlacementPayload | null;
  /** 「知道了，下次不要自动打开」——只关掉强制唤起，不动用户的 start_minimized。 */
  onDismiss: () => void;
  onOpenSettings: () => void;
}

/**
 * macOS 26 菜单栏准入被拒时的引导横幅。
 *
 * 为什么需要它：状态项被系统拒绝放置时**没有任何错误**——菜单建得起来、
 * 点击回调也正常，只是系统不给它位置。用户看到的是「应用装了但图标没了」，
 * 而第一反应几乎必然是去重装（本次排查就正是这样绕了一圈）。这条横幅的
 * 全部价值就是把这个哑故障翻译成一句人话加一条可执行路径。
 *
 * 配色刻意用琥珀而不是 connError 的玫红：那是「连不上服务器」这类功能中断，
 * 而这里应用其实完全正常，只是少了一个入口。同色会让用户以为传输也坏了。
 */
export const MenuBarHiddenBanner: React.FC<Props> = ({ payload, onDismiss, onOpenSettings }) => {
  // placed / unknown 都不显示。
  // unknown 是「判据失效」而非「真被拒」，甩给用户一个查不出所以然的告警
  // 比不告警更糟——这条判断与后端 should_warn_user 是同一个口径。
  if (!payload || payload.placement !== "rejected") return null;

  return (
    <div className="bg-amber-950/80 border-b border-amber-800/80 px-4 py-2 text-xs text-amber-200">
      {/*
        min-w-0 不可省：flex 子项的 min-width 默认是 auto，不加它下面那几行
        操作路径不会换行，只会横向溢出把后半句吃掉——connError 横幅上踩过这个坑。
      */}
      <div className="flex items-start justify-between gap-2">
        <div className="flex items-start gap-2 min-w-0">
          <MonitorX className="w-4 h-4 text-amber-400 shrink-0 mt-0.5" />
          <div className="min-w-0 space-y-1">
            <p className="font-medium break-words">菜单栏图标未能显示</p>
            <p className="break-words text-amber-200/80">
              macOS 26 (Tahoe) 新增了菜单栏准入控制。图标已经创建成功，但系统没有分配给它位置。
            </p>
            <ol className="list-decimal pl-4 space-y-0.5 text-amber-200/80 break-words">
              <li>
                打开 <span className="text-amber-100">系统设置 → 菜单栏 →「允许在菜单栏中」</span>
                ，找到 <span className="text-amber-100">瞬贴 (UniDrop)</span> 并确认开关打开。
                列表要等应用运行 1–2 分钟才会写入条目，稍候再看。
              </li>
              <li>
                开关<span className="text-amber-100">已经是打开</span>仍然不显示：这是 macOS
                把本应用拉进了会话级黑名单（应用已自动清理其已知诱因，但解除要靠系统重置）。
                <span className="text-amber-100">注销并重新登录（或重启电脑）</span>
                后图标即恢复，且通常不会再复发。
              </li>
            </ol>
            <p className="break-words text-amber-200/60">
              期间点击 Dock 栏的瞬贴图标随时可以唤回本窗口，功能不受影响。图标恢复后本提示会自行消失。
              {payload.start_minimized && " 你开了「启动即最小化」，图标恢复前窗口会在启动几秒后自动打开，以免你完全没有入口。"}
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
          {/* 仅在「会自动弹窗」时才给退出口：没开 start_minimized 的用户本来
              就不会被自动打扰，给个关不掉任何东西的按钮只会让人困惑。 */}
          {payload.start_minimized && !payload.force_reveal_optout && (
            <button
              onClick={onDismiss}
              className="px-2 py-1 rounded bg-amber-900/40 hover:bg-amber-800/40 text-amber-200/80 transition-colors whitespace-nowrap"
            >
              下次不要自动打开
            </button>
          )}
        </div>
      </div>
    </div>
  );
};
