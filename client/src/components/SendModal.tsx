import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { X, Send, Plus, Trash2, FileText, FolderPlus, AlertCircle } from "lucide-react";
import { OnlineDevice } from "../types";

interface SendModalProps {
  targetDevice: OnlineDevice;
  isOpen: boolean;
  onClose: () => void;
  onSuccess: (sessionId: string, count: number) => void;
}

export const SendModal: React.FC<SendModalProps> = ({
  targetDevice,
  isOpen,
  onClose,
  onSuccess,
}) => {
  const [paths, setPaths] = useState<string[]>([]);
  const [inputPath, setInputPath] = useState("");
  const [isSending, setIsSending] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);

  // Listen to Tauri native window drag & drop
  useEffect(() => {
    if (!isOpen) return;

    let unlisten: (() => void) | undefined;
    const setupDragDrop = async () => {
      try {
        const appWindow = getCurrentWebviewWindow();
        unlisten = await appWindow.onDragDropEvent((event) => {
          if (event.payload.type === "drop") {
            const droppedPaths = event.payload.paths;
            if (droppedPaths && droppedPaths.length > 0) {
              setPaths((prev) => {
                const set = new Set([...prev, ...droppedPaths]);
                return Array.from(set);
              });
            }
          }
        });
      } catch (err) {
        console.warn("Could not register drag-drop event listener:", err);
      }
    };

    setupDragDrop();
    return () => {
      if (unlisten) unlisten();
    };
  }, [isOpen]);

  if (!isOpen) return null;

  const handleAddPath = () => {
    const trimmed = inputPath.trim();
    if (!trimmed) return;
    const candidates = trimmed.split("\n").map((p) => p.trim()).filter(Boolean);
    setPaths((prev) => {
      const set = new Set([...prev, ...candidates]);
      return Array.from(set);
    });
    setInputPath("");
    setErrorMsg(null);
  };

  const handleRemovePath = (index: number) => {
    setPaths((prev) => prev.filter((_, i) => i !== index));
  };

  const handleSend = async () => {
    if (paths.length === 0) {
      setErrorMsg("请至少添加一个文件或目录路径");
      return;
    }

    setIsSending(true);
    setErrorMsg(null);

    try {
      const sessionId = await invoke<string>("cmd_send_files", {
        targetDevice: targetDevice.device_id,
        paths: paths,
      });
      onSuccess(sessionId, paths.length);
      onClose();
    } catch (err: any) {
      setErrorMsg(typeof err === "string" ? err : err.message || "发送请求失败");
    } finally {
      setIsSending(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div className="bg-slate-900 border border-slate-700 rounded-2xl w-full max-w-md p-5 shadow-2xl flex flex-col max-h-[90vh]">
        {/* Header */}
        <div className="flex items-center justify-between pb-3 border-b border-slate-800">
          <div className="flex items-center space-x-2">
            <Send className="w-5 h-5 text-teal-400" />
            <div>
              <h3 className="font-semibold text-slate-100 text-sm">发送文件</h3>
              <p className="text-[11px] text-slate-400">
                目标设备: <span className="text-teal-300 font-medium">{targetDevice.hostname}</span> ({targetDevice.os_type.toUpperCase()})
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="text-slate-400 hover:text-slate-200 p-1 rounded-lg hover:bg-slate-800 transition"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Body */}
        <div className="mt-4 space-y-3.5 flex-1 overflow-y-auto">
          {/* Error Banner */}
          {errorMsg && (
            <div className="p-2.5 rounded-lg bg-rose-500/20 border border-rose-500/40 flex items-center space-x-2 text-rose-300 text-xs">
              <AlertCircle className="w-4 h-4 shrink-0" />
              <span className="truncate">{errorMsg}</span>
            </div>
          )}

          {/* Path Input Box */}
          <div>
            <label className="block text-slate-400 text-xs mb-1.5 font-medium">
              输入文件或文件夹绝对路径 (支持拖拽文件到窗口)
            </label>
            <div className="flex space-x-2">
              <input
                type="text"
                value={inputPath}
                onChange={(e) => setInputPath(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    handleAddPath();
                  }
                }}
                placeholder="/path/to/file 或 C:\path\to\file"
                className="flex-1 bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-xs text-slate-200 placeholder-slate-500 focus:outline-none focus:border-teal-500"
              />
              <button
                type="button"
                onClick={handleAddPath}
                className="px-3 py-2 bg-slate-800 hover:bg-slate-700 border border-slate-700 rounded-lg text-slate-200 text-xs font-medium flex items-center space-x-1 transition"
              >
                <Plus className="w-3.5 h-3.5" />
                <span>添加</span>
              </button>
            </div>
          </div>

          {/* Drag & Drop Hint Box */}
          <div className="border-2 border-dashed border-slate-700/80 rounded-xl p-3 text-center bg-slate-850/40">
            <FolderPlus className="w-6 h-6 text-slate-500 mx-auto mb-1" />
            <p className="text-[11px] text-slate-400">将文件或文件夹直接拖动至窗口即可添加</p>
          </div>

          {/* Selected Paths List */}
          <div>
            <div className="flex items-center justify-between mb-1.5 text-xs">
              <span className="text-slate-400 font-medium">待发送列表 ({paths.length})</span>
              {paths.length > 0 && (
                <button
                  onClick={() => setPaths([])}
                  className="text-slate-500 hover:text-rose-400 text-[11px] transition"
                >
                  清空列表
                </button>
              )}
            </div>
            {paths.length === 0 ? (
              <div className="py-4 text-center text-slate-500 text-xs bg-slate-800/30 rounded-lg border border-slate-800">
                尚未添加任何文件
              </div>
            ) : (
              <div className="max-h-36 overflow-y-auto space-y-1.5 pr-1">
                {paths.map((p, idx) => (
                  <div
                    key={idx}
                    className="flex items-center justify-between px-2.5 py-1.5 rounded-lg bg-slate-800 border border-slate-700/70 text-xs text-slate-300"
                  >
                    <div className="flex items-center space-x-2 truncate mr-2">
                      <FileText className="w-3.5 h-3.5 text-teal-400 shrink-0" />
                      <span className="truncate font-mono text-[11px]">{p}</span>
                    </div>
                    <button
                      onClick={() => handleRemovePath(idx)}
                      className="text-slate-400 hover:text-rose-400 p-1 transition"
                      title="移除"
                    >
                      <Trash2 className="w-3.5 h-3.5" />
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* Footer */}
        <div className="pt-4 border-t border-slate-800 flex space-x-2 mt-3">
          <button
            type="button"
            onClick={onClose}
            disabled={isSending}
            className="flex-1 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-medium transition disabled:opacity-50"
          >
            取消
          </button>
          <button
            type="button"
            onClick={handleSend}
            disabled={isSending || paths.length === 0}
            className="flex-1 flex items-center justify-center space-x-1.5 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 text-white text-xs font-medium transition disabled:opacity-50 disabled:cursor-not-allowed"
          >
            <Send className="w-3.5 h-3.5" />
            <span>{isSending ? "正在哈希并打包..." : `开始发送 (${paths.length})`}</span>
          </button>
        </div>
      </div>
    </div>
  );
};
