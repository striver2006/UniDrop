import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Settings, RefreshCw, ShieldCheck, Clipboard } from "lucide-react";
import { OnlineDevice, AppSettings, ActiveTransfer } from "./types";
import { DeviceList } from "./components/DeviceList";
import { TransferProgress } from "./components/TransferProgress";
import { SettingsModal } from "./components/SettingsModal";

export const App: React.FC = () => {
  const [selfDevice, setSelfDevice] = useState<OnlineDevice | null>(null);
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [transfers] = useState<ActiveTransfer[]>([]);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);

  const fetchInitialData = async () => {
    setIsRefreshing(true);
    try {
      const self = await invoke<OnlineDevice>("cmd_get_self_info");
      setSelfDevice(self);

      const list = await invoke<OnlineDevice[]>("cmd_get_online_devices");
      setDevices(list);

      const s = await invoke<AppSettings>("cmd_get_settings");
      setSettings(s);
    } catch (err) {
      console.error("fetch initial data error:", err);
    } finally {
      setIsRefreshing(false);
    }
  };

  useEffect(() => {
    fetchInitialData();
  }, []);

  const handleSaveSettings = async (newSettings: AppSettings) => {
    try {
      await invoke("cmd_save_settings", { newSettings });
      setSettings(newSettings);
    } catch (err) {
      console.error("save settings error:", err);
    }
  };

  const handleSendToDevice = (target: OnlineDevice) => {
    alert(`准备发送至设备: ${target.hostname}`);
  };

  return (
    <div className="flex flex-col h-screen w-full bg-slate-900 border border-slate-800 rounded-2xl overflow-hidden shadow-2xl">
      {/* Header */}
      <header className="flex items-center justify-between px-4 py-3 bg-slate-850/80 backdrop-blur border-b border-slate-800">
        <div className="flex items-center space-x-2">
          <div className="w-7 h-7 rounded-lg bg-teal-500/20 flex items-center justify-center border border-teal-500/40">
            <Clipboard className="w-4 h-4 text-teal-400" />
          </div>
          <div>
            <h1 className="text-sm font-semibold text-slate-100 flex items-center space-x-1.5">
              <span>UniDrop</span>
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-teal-500/20 text-teal-300 font-normal">
                v0.1.0
              </span>
            </h1>
            <p className="text-[11px] text-slate-400">
              本机: {selfDevice ? `${selfDevice.hostname} (${selfDevice.os_type.toUpperCase()})` : "载入中..."}
            </p>
          </div>
        </div>

        <div className="flex items-center space-x-1">
          <button
            onClick={fetchInitialData}
            className={`p-1.5 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition ${
              isRefreshing ? "animate-spin text-teal-400" : ""
            }`}
            title="刷新设备列表"
          >
            <RefreshCw className="w-4 h-4" />
          </button>
          <button
            onClick={() => setIsSettingsOpen(true)}
            className="p-1.5 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition"
            title="偏好设置"
          >
            <Settings className="w-4 h-4" />
          </button>
        </div>
      </header>

      {/* Main Content */}
      <main className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* Active Transfers */}
        <TransferProgress transfers={transfers} />

        {/* Online Devices Section */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-slate-400">在线协同设备</h3>
            <span className="text-xs text-slate-500">{devices.length} 台可用</span>
          </div>
          <DeviceList devices={devices} onSendToDevice={handleSendToDevice} />
        </div>
      </main>

      {/* Footer Info */}
      <footer className="px-4 py-2 bg-slate-950/60 border-t border-slate-800/80 flex items-center justify-between text-[11px] text-slate-500">
        <div className="flex items-center space-x-1 text-teal-500/90">
          <ShieldCheck className="w-3.5 h-3.5" />
          <span>PSK 接入安全就绪</span>
        </div>
        <span>支持 Ctrl+V / Cmd+V 原生落地</span>
      </footer>

      {/* Settings Modal */}
      {settings && (
        <SettingsModal
          settings={settings}
          isOpen={isSettingsOpen}
          onClose={() => setIsSettingsOpen(false)}
          onSave={handleSaveSettings}
        />
      )}
    </div>
  );
};
