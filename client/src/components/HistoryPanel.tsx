import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ArrowDownCircle,
  ArrowUpCircle,
  CheckCircle2,
  AlertCircle,
  Clipboard,
  Save,
  FolderOpen,
  RefreshCw,
  History as HistoryIcon,
} from "lucide-react";
import { ActiveTransfer, TransferHistoryEntry } from "../types";

interface HistoryPanelProps {
  onNotify: (text: string, type?: "info" | "success" | "error") => void;
}

export const HistoryPanel: React.FC<HistoryPanelProps> = ({ onNotify }) => {
  const [entries, setEntries] = useState<TransferHistoryEntry[]>([]);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [busySession, setBusySession] = useState<string | null>(null);

  const refresh = useCallback(async (silent = false) => {
    if (!silent) setIsRefreshing(true);
    try {
      const list = await invoke<TransferHistoryEntry[]>("cmd_list_history");
      setEntries(Array.isArray(list) ? list : []);
    } catch (err) {
      console.error("list history error:", err);
    } finally {
      if (!silent) setIsRefreshing(false);
    }
  }, []);

  useEffect(() => {
    refresh();
    // Silent refresh whenever a transfer finishes
    const unlistenPromise = listen<ActiveTransfer>("transfer-progress", (event) => {
      if (event.payload?.status === "COMPLETED" || event.payload?.status === "FAILED") {
        refresh(true);
      }
    });
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, [refresh]);

  const handleInject = async (entry: TransferHistoryEntry) => {
    setBusySession(entry.session_id);
    try {
      const msg = await invoke<string>("cmd_inject_session", { sessionId: entry.session_id });
      onNotify(msg || "已写入系统剪贴板", "success");
    } catch (err: any) {
      onNotify(typeof err === "string" ? err : "写入剪贴板失败", "error");
    } finally {
      setBusySession(null);
    }
  };

  const handleSaveAs = async (entry: TransferHistoryEntry) => {
    setBusySession(entry.session_id);
    try {
      const copied = await invoke<number | null>("cmd_save_transfer_as", {
        sessionId: entry.session_id,
      });
      if (copied === null) {
        onNotify("已取消另存为", "info");
      } else {
        onNotify(`已另存 ${copied} 个文件到所选文件夹`, "success");
      }
    } catch (err: any) {
      onNotify(typeof err === "string" ? err : "另存为失败", "error");
    } finally {
      setBusySession(null);
    }
  };

  const handleReveal = async (entry: TransferHistoryEntry) => {
    try {
      await invoke("cmd_reveal_session", { sessionId: entry.session_id });
    } catch (err: any) {
      onNotify(typeof err === "string" ? err : "打开文件位置失败", "error");
    }
  };

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  const formatTime = (raw: string | null) => {
    if (!raw) return "";
    try {
      // SQLite CURRENT_TIMESTAMP is UTC "YYYY-MM-DD HH:MM:SS"
      const d = new Date(raw.replace(" ", "T") + "Z");
      if (isNaN(d.getTime())) return raw;
      return d.toLocaleString(undefined, {
        month: "2-digit",
        day: "2-digit",
        hour: "2-digit",
        minute: "2-digit",
      });
    } catch {
      return raw;
    }
  };

  const typeLabel = (t: string) => (t === "TEXT" ? "文本" : t === "IMAGE" ? "图片" : "文件");

  return (
    <div>
      <div className="flex items-center justify-between mb-2">
        <h3 className="flex items-center space-x-1.5 text-xs font-semibold uppercase tracking-wider text-slate-400">
          <HistoryIcon className="w-3.5 h-3.5" />
          <span>传输历史</span>
        </h3>
        <button
          onClick={() => refresh()}
          className={`p-1 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition ${
            isRefreshing ? "animate-spin text-teal-400" : ""
          }`}
          title="刷新历史"
        >
          <RefreshCw className="w-3.5 h-3.5" />
        </button>
      </div>

      {entries.length === 0 ? (
        <div className="py-4 text-center text-slate-500 text-xs bg-slate-800/30 rounded-lg border border-slate-800">
          暂无传输历史
        </div>
      ) : (
        <div className="max-h-64 overflow-y-auto space-y-1.5 pr-1">
          {entries.map((entry) => {
            const isDone = entry.status === "COMPLETED";
            const isFailed = entry.status === "FAILED";
            const hasCache = entry.cached_count > 0 && isDone;
            const busy = busySession === entry.session_id;

            return (
              <div
                key={entry.session_id}
                className={`px-2.5 py-2 rounded-lg border text-xs ${
                  isDone
                    ? "border-slate-700/70 bg-slate-800/60"
                    : isFailed
                    ? "border-rose-500/30 bg-rose-950/20"
                    : "border-teal-500/30 bg-slate-800/60"
                }`}
              >
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center space-x-1.5 min-w-0">
                    {entry.direction === "SEND" ? (
                      <ArrowUpCircle className="w-3.5 h-3.5 text-teal-400 shrink-0" />
                    ) : (
                      <ArrowDownCircle className="w-3.5 h-3.5 text-sky-400 shrink-0" />
                    )}
                    <span className="truncate text-slate-200" title={entry.preview_summary || ""}>
                      {entry.preview_summary || "传输"}
                    </span>
                    <span className="text-[10px] text-slate-500 shrink-0">
                      {typeLabel(entry.data_type)} · {formatBytes(entry.total_size)}
                    </span>
                  </div>
                  <div className="flex items-center space-x-1.5 shrink-0">
                    {isDone && (
                      <span className="flex items-center text-[10px] text-emerald-400">
                        <CheckCircle2 className="w-3 h-3 mr-0.5" />
                        完成
                      </span>
                    )}
                    {isFailed && (
                      <span
                        className="flex items-center text-[10px] text-rose-400"
                        title={entry.error_message || ""}
                      >
                        <AlertCircle className="w-3 h-3 mr-0.5" />
                        失败
                      </span>
                    )}
                    {!isDone && !isFailed && (
                      <span className="text-[10px] text-teal-300">进行中</span>
                    )}
                    <span className="text-[10px] text-slate-500 font-mono">
                      {formatTime(entry.created_at)}
                    </span>
                  </div>
                </div>

                {entry.direction === "RECEIVE" && (
                  <div className="flex items-center justify-end space-x-1.5 mt-1.5">
                    {hasCache ? (
                      <>
                        <button
                          onClick={() => handleInject(entry)}
                          disabled={busy}
                          className="flex items-center space-x-1 px-2 py-0.5 rounded-md bg-teal-600/25 hover:bg-teal-600/45 text-teal-300 text-[10px] font-medium transition disabled:opacity-40"
                          title="写入系统剪贴板"
                        >
                          <Clipboard className="w-3 h-3" />
                          <span>{entry.data_type === "TEXT" ? "复制文本" : entry.data_type === "IMAGE" ? "复制图片" : "装载"}</span>
                        </button>
                        <button
                          onClick={() => handleSaveAs(entry)}
                          disabled={busy}
                          className="flex items-center space-x-1 px-2 py-0.5 rounded-md bg-slate-700/60 hover:bg-slate-700 text-slate-300 text-[10px] font-medium transition disabled:opacity-40"
                          title="另存到指定文件夹"
                        >
                          <Save className="w-3 h-3" />
                          <span>另存为</span>
                        </button>
                        <button
                          onClick={() => handleReveal(entry)}
                          disabled={busy}
                          className="flex items-center space-x-1 px-2 py-0.5 rounded-md bg-slate-700/60 hover:bg-slate-700 text-slate-300 text-[10px] font-medium transition disabled:opacity-40"
                          title="在文件管理器中显示"
                        >
                          <FolderOpen className="w-3 h-3" />
                          <span>打开位置</span>
                        </button>
                      </>
                    ) : isDone ? (
                      <span className="text-[10px] text-slate-500">缓存已清理，无法重新装载</span>
                    ) : null}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
