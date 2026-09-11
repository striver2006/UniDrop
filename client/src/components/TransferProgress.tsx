import React from "react";
import { ArrowDownCircle, ArrowUpCircle } from "lucide-react";
import { ActiveTransfer } from "../types";

interface TransferProgressProps {
  transfers: ActiveTransfer[];
}

export const TransferProgress: React.FC<TransferProgressProps> = ({ transfers }) => {
  if (transfers.length === 0) return null;

  const formatBytes = (bytes: number) => {
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div className="space-y-2 mb-4">
      {transfers.map((t) => (
        <div key={t.session_id} className="p-3 rounded-xl bg-slate-800 border border-teal-500/30">
          <div className="flex items-center justify-between text-xs mb-1.5">
            <div className="flex items-center space-x-1.5 font-medium text-slate-200">
              {t.direction === "SEND" ? (
                <ArrowUpCircle className="w-4 h-4 text-teal-400" />
              ) : (
                <ArrowDownCircle className="w-4 h-4 text-sky-400" />
              )}
              <span className="truncate max-w-[180px]">{t.preview_summary}</span>
            </div>
            <span className="text-slate-400">
              {formatBytes(t.transferred_size)} / {formatBytes(t.total_size)}
            </span>
          </div>

          <div className="w-full h-1.5 bg-slate-700 rounded-full overflow-hidden">
            <div
              className="h-full bg-teal-500 rounded-full transition-all duration-200"
              style={{ width: `${t.progress}%` }}
            />
          </div>
        </div>
      ))}
    </div>
  );
};
