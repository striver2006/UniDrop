import React from "react";
import { ArrowDownCircle, ArrowUpCircle, CheckCircle2, AlertCircle, X, Clipboard } from "lucide-react";
import { ActiveTransfer } from "../types";

interface TransferProgressProps {
  transfers: ActiveTransfer[];
  onDismiss?: (sessionId: string) => void;
  onInject?: (sessionId: string) => void;
}

export const TransferProgress: React.FC<TransferProgressProps> = ({ transfers = [], onDismiss, onInject }) => {
  const safeTransfers = Array.isArray(transfers) ? transfers : [];
  if (safeTransfers.length === 0) return null;

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div className="space-y-2 mb-4">
      <div className="flex items-center justify-between text-xs text-slate-400 font-medium">
        <span>传输任务 ({safeTransfers.length})</span>
      </div>
      {safeTransfers.map((t) => {
        const isDone = t.status === "COMPLETED";
        const isFailed = t.status === "FAILED";

        return (
          <div
            key={t.session_id}
            className={`p-3 rounded-xl bg-slate-800 border transition ${
              isDone
                ? "border-emerald-500/40 bg-emerald-950/20"
                : isFailed
                ? "border-rose-500/40 bg-rose-950/20"
                : "border-teal-500/40"
            }`}
          >
            <div className="flex items-center justify-between text-xs mb-1.5">
              <div className="flex items-center space-x-1.5 font-medium text-slate-200">
                {t.direction === "SEND" ? (
                  <ArrowUpCircle className="w-4 h-4 text-teal-400 shrink-0" />
                ) : (
                  <ArrowDownCircle className="w-4 h-4 text-sky-400 shrink-0" />
                )}
                <span className="truncate max-w-[170px] text-xs" title={t.preview_summary}>
                  {t.preview_summary || "文件流传输"}
                </span>
              </div>

              <div className="flex items-center space-x-2">
                {isDone && (
                  <span className="flex items-center space-x-1 text-[11px] text-emerald-400">
                    <CheckCircle2 className="w-3.5 h-3.5" />
                    <span>已完成</span>
                  </span>
                )}
                {isFailed && (
                  <span className="flex items-center space-x-1 text-[11px] text-rose-400">
                    <AlertCircle className="w-3.5 h-3.5" />
                    <span>传输失败</span>
                  </span>
                )}
                {!isDone && !isFailed && (
                  <span className="text-[11px] font-mono text-teal-300">
                    {Math.round(t.progress)}%
                  </span>
                )}
                {onDismiss && (isDone || isFailed) && (
                  <button
                    onClick={() => onDismiss(t.session_id)}
                    className="text-slate-500 hover:text-slate-300 p-0.5"
                    title="移除"
                  >
                    <X className="w-3 h-3" />
                  </button>
                )}
              </div>
            </div>

            <div className="w-full h-1.5 bg-slate-700/70 rounded-full overflow-hidden mb-1">
              <div
                className={`h-full rounded-full transition-all duration-300 ${
                  isDone
                    ? "bg-emerald-500"
                    : isFailed
                    ? "bg-rose-500"
                    : "bg-teal-500"
                }`}
                style={{ width: `${Math.min(100, Math.max(0, t.progress))}%` }}
              />
            </div>

            <div className="flex justify-between items-center text-[10px] text-slate-400 font-mono">
              <span>{t.direction === "SEND" ? "发送" : "接收"}</span>
              <span>
                {formatBytes(t.transferred_size)} / {formatBytes(t.total_size)}
              </span>
            </div>

            {/* M1: Manual inject button for completed receive tasks */}
            {t.direction === "RECEIVE" && isDone && onInject && (
              <div className="mt-2 pt-2 border-t border-slate-700/60 flex items-center justify-between">
                <span className="text-[11px] text-emerald-400/90 flex items-center space-x-1">
                  <CheckCircle2 className="w-3 h-3" />
                  <span>
                    {t.data_type === "TEXT"
                      ? "文本已写入剪贴板"
                      : t.data_type === "IMAGE"
                      ? "图片已写入剪贴板"
                      : "文件已就绪"}
                  </span>
                </span>
                <button
                  type="button"
                  onClick={() => onInject(t.session_id)}
                  className="flex items-center space-x-1 px-2.5 py-1 rounded-lg bg-teal-600/30 hover:bg-teal-600/50 text-teal-300 text-[11px] font-medium transition duration-150 cursor-pointer"
                  title="重新写入系统剪贴板"
                >
                  <Clipboard className="w-3 h-3" />
                  <span>
                    {t.data_type === "TEXT"
                      ? "重新复制文本"
                      : t.data_type === "IMAGE"
                      ? "重新复制图片"
                      : "装载到剪贴板"}
                  </span>
                </button>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
};
