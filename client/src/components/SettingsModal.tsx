import React, { useState, useEffect } from "react";
import { X, Save, Shield } from "lucide-react";
import { AppSettings } from "../types";

interface SettingsModalProps {
  settings: AppSettings;
  isOpen: boolean;
  onClose: () => void;
  onSave: (newSettings: AppSettings) => void;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({ settings, isOpen, onClose, onSave }) => {
  const [form, setForm] = useState<AppSettings>(settings);

  useEffect(() => {
    setForm(settings);
  }, [settings, isOpen]);

  if (!isOpen) return null;

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    let cleanUrl = form.server_url.replace(/\s+/g, "");
    if (cleanUrl && !cleanUrl.startsWith("ws://") && !cleanUrl.startsWith("wss://")) {
      cleanUrl = `wss://${cleanUrl}`;
    }
    onSave({
      ...form,
      server_url: cleanUrl,
      account_id: form.account_id.trim(),
      psk_secret: form.psk_secret.trim(),
    });
    onClose();
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
              className="flex-1 flex items-center justify-center space-x-1.5 py-2 rounded-lg bg-teal-600 hover:bg-teal-500 text-white font-medium transition"
            >
              <Save className="w-4 h-4" />
              <span>保存配置</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
