import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Save, Shield, AlertCircle } from "lucide-react";
import { AppSettings } from "../types";

interface SettingsModalProps {
  settings: AppSettings;
  isOpen: boolean;
  onClose: () => void;
  onSave: (newSettings: AppSettings) => void | Promise<void>;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ settings, isOpen, onClose, onSave }) => {
  const [form, setForm] = useState<AppSettings>(settings);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 开机自启不属于 AppSettings：它的事实源是操作系统，本地不留副本，
  // 因此独立拉取、独立写入，与表单保存的失败域互不污染。
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);

  useEffect(() => {
    setForm(settings);
    setError(null);
  }, [settings, isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    // 每次打开都读操作系统实时状态：用户可能在系统设置里改过它。
    // 拉取期间禁用开关，否则组件初值 false 会让用户对着尚未确定的状态点击；
    // cancelled 标志丢弃过期回调，避免快速关开弹窗时迟到的响应覆盖更新的值。
    let cancelled = false;
    setAutostartError(null);
    setAutostartBusy(true);
    invoke<boolean>("cmd_get_autostart")
      .then((enabled) => {
        if (!cancelled) setAutostart(enabled);
      })
      .catch((err: any) => {
        console.error("get autostart error:", err);
        if (!cancelled) {
          setAutostartError(typeof err === "string" ? err : "读取开机自启状态失败");
        }
      })
      .finally(() => {
        if (!cancelled) setAutostartBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen]);

  if (!isOpen) return null;

  const handleAutostartChange = async (next: boolean) => {
    const previous = autostart;
    setAutostart(next);
    setAutostartBusy(true);
    setAutostartError(null);
    try {
      await invoke("cmd_set_autostart", { enabled: next });
    } catch (err: any) {
      console.error("set autostart error:", err);
      setAutostartError(typeof err === "string" ? err : "设置开机自启失败");
      // 回滚到 OS 的真实状态而非本地快照：快照可能是上一次过期拉取留下的
      try {
        setAutostart(await invoke<boolean>("cmd_get_autostart"));
      } catch (readErr: any) {
        console.error("re-read autostart error:", readErr);
        setAutostart(previous);
      }
    } finally {
      setAutostartBusy(false);
    }
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (saving) return;

    let cleanUrl = form.server_url.replace(/\s+/g, "");
    if (cleanUrl && !cleanUrl.startsWith("ws://") && !cleanUrl.startsWith("wss://")) {
      cleanUrl = `wss://${cleanUrl}`;
    }

    setSaving(true);
    setError(null);
    try {
      await onSave({
        ...form,
        server_url: cleanUrl,
        account_id: form.account_id.trim(),
        psk_secret: form.psk_secret.trim(),
      });
      onClose(); // 只有保存成功才关窗，失败时保留用户已填内容
    } catch (err: any) {
      setError(typeof err === "string" ? err : "保存设置失败，请检查后重试");
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/60 backdrop-blur-sm flex items-center justify-center p-4 z-50">
      <div className="bg-slate-900 border border-slate-700 rounded-2xl w-full max-w-sm p-5 shadow-2xl">
        <div className="flex items-center justify-between pb-3 border-b border-slate-800">
          <div className="flex items-center space-x-2">
            <Shield className="w-5 h-5 text-teal-400" />
            <h3 className="font-semibold text-slate-100">连接与安全偏好</h3>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="mt-4 space-y-3.5 text-xs">
          <div>
            <label className="block text-slate-400 mb-1">公网中继服务器地址 (WSS/WS)</label>
            <input
              type="text"
              value={form.server_url}
              onChange={(e) => setForm({ ...form, server_url: e.target.value })}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
              placeholder="wss://drop.yourdomain.com:58921"
              required
            />
          </div>

          <div>
            <label className="block text-slate-400 mb-1">账号标识 (Account ID)</label>
            <input
              type="text"
              value={form.account_id}
              onChange={(e) => setForm({ ...form, account_id: e.target.value })}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
              required
            />
          </div>

          <div>
            <label className="block text-slate-400 mb-1">预共享密钥 (PSK Secret Key)</label>
            <input
              type="password"
              value={form.psk_secret}
              onChange={(e) => setForm({ ...form, psk_secret: e.target.value })}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
              required
            />
          </div>

          <div className="flex items-center justify-between pt-2">
            <div>
              <span className="font-medium text-slate-200">静默自动装载剪贴板</span>
              <p className="text-[11px] text-slate-500">免去点击系统通知，直接写入系统剪贴板</p>
            </div>
            <input
              type="checkbox"
              checked={form.auto_inject}
              onChange={(e) => setForm({ ...form, auto_inject: e.target.checked })}
              className="w-4 h-4 rounded text-teal-500 focus:ring-teal-400 bg-slate-800 border-slate-700"
            />
          </div>

          <div className="flex items-center justify-between pt-2">
            <div>
              <span className="font-medium text-slate-200">开机自动启动</span>
              <p className="text-[11px] text-slate-500">登录系统后自动运行，修改立即生效</p>
            </div>
            <input
              type="checkbox"
              checked={autostart}
              disabled={autostartBusy}
              onChange={(e) => handleAutostartChange(e.target.checked)}
              className="w-4 h-4 rounded text-teal-500 focus:ring-teal-400 bg-slate-800 border-slate-700 disabled:opacity-50"
            />
          </div>

          {autostartError && (
            <div className="flex items-start space-x-1.5 text-[11px] text-amber-400">
              <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
              <span>{autostartError}</span>
            </div>
          )}

          <div className="flex items-center justify-between pt-2">
            <div>
              <span className="font-medium text-slate-200">启动时最小化到托盘</span>
              <p className="text-[11px] text-slate-500">启动后不弹出主窗口；点击托盘图标可随时唤起</p>
            </div>
            <input
              type="checkbox"
              checked={form.start_minimized}
              onChange={(e) => setForm({ ...form, start_minimized: e.target.checked })}
              className="w-4 h-4 rounded text-teal-500 focus:ring-teal-400 bg-slate-800 border-slate-700"
            />
          </div>

          {error && (
            <div className="flex items-start space-x-1.5 text-[11px] text-rose-400">
              <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <div className="pt-3 flex space-x-2">
            <button
              type="button"
              onClick={onClose}
              className="flex-1 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 font-medium transition"
            >
              取消
            </button>
            <button
              type="submit"
              disabled={saving}
              className="flex-1 flex items-center justify-center space-x-1.5 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 disabled:bg-teal-800 disabled:cursor-not-allowed text-white font-medium transition"
            >
              <Save className="w-4 h-4" />
              <span>{saving ? "保存中..." : "保存配置"}</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
