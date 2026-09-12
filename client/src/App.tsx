import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import {
  Settings,
  RefreshCw,
  ShieldCheck,
  ShieldAlert,
  Clipboard,
  CheckCircle2,
  AlertCircle,
  X,
} from "lucide-react";
import { OnlineDevice, AppSettings, ActiveTransfer } from "./types";
import { DeviceList } from "./components/DeviceList";
import { TransferProgress } from "./components/TransferProgress";
import { SettingsModal } from "./components/SettingsModal";
import { SendModal } from "./components/SendModal";
import { HistoryPanel } from "./components/HistoryPanel";

const defaultSettings: AppSettings = {
  server_url: "wss://drop.yourdomain.com:58921",
  account_id: "default_user",
  psk_secret: "dev-insecure-psk-secret",
  auto_inject: false,
  rate_limit_mb: 10,
};

export const App: React.FC = () => {
  const [selfDevice, setSelfDevice] = useState<OnlineDevice | null>(null);
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [transfers, setTransfers] = useState<ActiveTransfer[]>([]);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [selectedDeviceForSend, setSelectedDeviceForSend] = useState<OnlineDevice | null>(null);
  const [authError, setAuthError] = useState<string | null>(null);
  const [notification, setNotification] = useState<{
    type: "info" | "success" | "error";
    text: string;
  } | null>(null);

  const showNotification = (text: string, type: "info" | "success" | "error" = "info") => {
    setNotification({ text, type });
    setTimeout(() => {
      setNotification((curr) => (curr?.text === text ? null : curr));
    }, 4000);
  };

  const fetchInitialData = async () => {
    setIsRefreshing(true);
    try {
      const self = await invoke<OnlineDevice>("cmd_get_self_info");
      setSelfDevice(self);

      const list = await invoke<OnlineDevice[]>("cmd_get_online_devices");
      setDevices(list);

      const s = await invoke<AppSettings>("cmd_get_settings");
      if (s.server_url === "ws://127.0.0.1:8080") {
        s.server_url = "wss://drop.yourdomain.com:58921";
      }
      setSettings(s);
    } catch (err: any) {
      console.error("fetch initial data error:", err);
      showNotification(typeof err === "string" ? err : "拉取初始状态失败", "error");
    } finally {
      setIsRefreshing(false);
    }
  };

  useEffect(() => {
    fetchInitialData();

    // 1. Listen for device list updates from control connection
    const unlistenDevicesPromise = listen<OnlineDevice[] | null>("devices-updated", async (event) => {
      if (Array.isArray(event.payload)) {
        setDevices(event.payload);
      } else {
        try {
          const list = await invoke<OnlineDevice[]>("cmd_get_online_devices");
          setDevices(Array.isArray(list) ? list : []);
        } catch (err) {
          console.error("fetch devices error:", err);
        }
      }
      setAuthError(null); // Devices updated implies successful authentication
    });

    // 2. Listen for auth events
    const unlistenAuthPromise = listen<string>("auth-failed", (event) => {
      setAuthError(event.payload);
      showNotification(`身份验证失败: ${event.payload}`, "error");
    });

    const unlistenAuthSuccessPromise = listen("auth-success", () => {
      setAuthError(null);
      showNotification("安全鉴权成功，已连接中继服务器", "success");
    });

    // 3. Listen for transfer progress updates (both outbound and inbound)
    const unlistenProgressPromise = listen<ActiveTransfer>("transfer-progress", (event) => {
      const update = event.payload;
      setTransfers((prev) => {
        const index = prev.findIndex((t) => t.session_id === update.session_id);
        if (index >= 0) {
          const updated = [...prev];
          updated[index] = update;
          return updated;
        } else {
          return [update, ...prev];
        }
      });

      if (update.status === "COMPLETED") {
        showNotification(`传输完成: ${update.preview_summary}`, "success");
      } else if (update.status === "FAILED") {
        showNotification(`传输失败: ${update.preview_summary}`, "error");
      }
    });

    // 4. Listen for inbound transfer offer notifications
    const unlistenOfferPromise = listen<any>("transfer-offer-received", (event) => {
      const payload = event.payload;
      const typeLabel =
        payload?.data_type === "TEXT" ? "文本" : payload?.data_type === "IMAGE" ? "图片" : "文件";
      const summary: string = payload?.preview_summary || "";
      showNotification(
        summary ? `收到${typeLabel}: ${summary}` : `收到${typeLabel}传输请求`,
        "info"
      );
    });

    // 5. Listen for open-settings event from system tray
    const unlistenSettingsPromise = listen("open-settings", () => {
      setIsSettingsOpen(true);
    });

    return () => {
      unlistenDevicesPromise.then((unlisten) => unlisten());
      unlistenAuthPromise.then((unlisten) => unlisten());
      unlistenAuthSuccessPromise.then((unlisten) => unlisten());
      unlistenProgressPromise.then((unlisten) => unlisten());
      unlistenOfferPromise.then((unlisten) => unlisten());
      unlistenSettingsPromise.then((unlisten) => unlisten());
    };
  }, []);

  const handleSaveSettings = async (newSettings: AppSettings) => {
    try {
      await invoke("cmd_save_settings", { newSettings });
      setSettings(newSettings);
      setAuthError(null);
      showNotification("配置已保存并即时生效，正在重连中继服务器...", "success");
      setTimeout(() => {
        fetchInitialData();
      }, 300);
    } catch (err: any) {
      console.error("save settings error:", err);
      showNotification(typeof err === "string" ? err : "保存设置失败", "error");
    }
  };

  const handleSendToDevice = (target: OnlineDevice) => {
    setSelectedDeviceForSend(target);
  };

  const handleTransferSent = (_sessionId: string, count: number) => {
    showNotification(`已发起发送请求 (${count} 个文件)，等待对方接收...`, "info");
  };

  const handleDismissTransfer = (sessionId: string) => {
    setTransfers((prev) => prev.filter((t) => t.session_id !== sessionId));
  };

  const handleInjectTransfer = async (sessionId: string) => {
    try {
      const msg = await invoke<string>("cmd_inject_session", { sessionId });
      showNotification(msg || "已装载至系统剪贴板，可直接按 Ctrl+V / Cmd+V 粘贴！", "success");
    } catch (err: any) {
      console.error("inject session error:", err);
      showNotification(typeof err === "string" ? err : "装载剪贴板失败", "error");
    }
  };

  return (
    <div className="flex flex-col h-screen w-full bg-slate-900 border border-slate-800 rounded-2xl overflow-hidden shadow-2xl select-none">
      {/* Draggable header. Keep a single drag path (data-tauri-drag-region):
          an extra manual start_dragging call on the same mousedown breaks
          dragging on Windows (second ReleaseCapture cancels the move loop). */}
      <header
        data-tauri-drag-region
        className="flex items-center justify-between px-4 py-3 bg-slate-850/80 backdrop-blur border-b border-slate-800 cursor-move select-none"
      >
        <div data-tauri-drag-region className="flex items-center space-x-2 pointer-events-auto">
          <div className="w-7 h-7 rounded-lg bg-teal-500/20 flex items-center justify-center border border-teal-500/40 pointer-events-none">
            <Clipboard className="w-4 h-4 text-teal-400" />
          </div>
          <div data-tauri-drag-region>
            <h1 className="text-sm font-semibold text-slate-100 flex items-center space-x-1.5 pointer-events-none">
              <span>瞬贴</span>
              <span className="text-[11px] text-slate-400 font-normal">UniDrop</span>
              <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-teal-500/20 text-teal-300 font-normal">
                v0.1.1
              </span>
            </h1>
            <p className="text-[11px] text-slate-400 pointer-events-none">
              本机: {selfDevice ? `${selfDevice.hostname} (${selfDevice.os_type.toUpperCase()})` : "载入中..."}
            </p>
          </div>
        </div>

        <div className="flex items-center space-x-1.5 cursor-default">
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
            className="flex items-center space-x-1 px-2.5 py-1 rounded-lg bg-slate-800/90 hover:bg-slate-700/90 border border-slate-700/70 text-slate-200 hover:text-teal-300 transition text-xs font-medium shadow-sm"
            title="偏好设置"
          >
            <Settings className="w-3.5 h-3.5 text-teal-400" />
            <span>偏好设置</span>
          </button>
          <button
            onClick={async () => {
              try {
                await invoke("cmd_hide_window");
              } catch (e) {
                console.warn("cmd_hide_window error, fallback to getCurrentWebviewWindow():", e);
                try {
                  const win = getCurrentWebviewWindow();
                  await win.hide();
                } catch (err) {
                  console.error("win.hide error:", err);
                }
              }
            }}
            className="p-1 rounded-lg text-slate-400 hover:text-slate-200 hover:bg-slate-800 transition ml-0.5"
            title="隐藏窗口 (保持后台常驻)"
          >
            <X className="w-4 h-4" />
          </button>
        </div>
      </header>

      {/* Auth Error Banner */}
      {authError && (
        <div className="bg-rose-950/80 border-b border-rose-800/80 px-4 py-2 flex items-center justify-between text-xs text-rose-300">
          <div className="flex items-center space-x-2 truncate">
            <ShieldAlert className="w-4 h-4 text-rose-400 shrink-0" />
            <span className="truncate">鉴权失败: {authError}</span>
          </div>
          <button
            onClick={() => setIsSettingsOpen(true)}
            className="text-[11px] underline hover:text-rose-200 font-medium shrink-0 ml-2"
          >
            检查密钥
          </button>
        </div>
      )}

      {/* Ephemeral Notification Toast */}
      {notification && (
        <div
          className={`px-4 py-1.5 text-xs flex items-center justify-between transition ${
            notification.type === "error"
              ? "bg-rose-900/70 text-rose-200 border-b border-rose-800"
              : notification.type === "success"
              ? "bg-emerald-900/70 text-emerald-200 border-b border-emerald-800"
              : "bg-teal-900/70 text-teal-200 border-b border-teal-800"
          }`}
        >
          <div className="flex items-center space-x-1.5 truncate">
            {notification.type === "error" ? (
              <AlertCircle className="w-3.5 h-3.5 shrink-0" />
            ) : (
              <CheckCircle2 className="w-3.5 h-3.5 shrink-0" />
            )}
            <span className="truncate">{notification.text}</span>
          </div>
          <button
            onClick={() => setNotification(null)}
            className="text-slate-400 hover:text-slate-200 ml-2"
          >
            <X className="w-3 h-3" />
          </button>
        </div>
      )}

      {/* Main Content */}
      <main className="flex-1 overflow-y-auto p-4 space-y-4">
        {/* Active Transfers */}
        <TransferProgress
          transfers={transfers}
          onDismiss={handleDismissTransfer}
          onInject={handleInjectTransfer}
        />

        {/* Online Devices Section */}
        <div>
          <div className="flex items-center justify-between mb-2">
            <h3 className="text-xs font-semibold uppercase tracking-wider text-slate-400">在线协同设备</h3>
            <span className="text-xs text-slate-500">{(devices || []).length} 台可用</span>
          </div>
          <DeviceList devices={devices || []} onSendToDevice={handleSendToDevice} />
        </div>

        {/* Transfer History */}
        <HistoryPanel onNotify={showNotification} />
      </main>

      {/* Footer Info */}
      <footer className="px-4 py-2 bg-slate-950/60 border-t border-slate-800/80 flex items-center justify-between text-[11px] text-slate-500">
        <div className={`flex items-center space-x-1 ${authError ? "text-rose-400" : "text-teal-500/90"}`}>
          {authError ? <ShieldAlert className="w-3.5 h-3.5" /> : <ShieldCheck className="w-3.5 h-3.5" />}
          <span>{authError ? "连接未授权" : "PSK 接入安全就绪"}</span>
        </div>
        <span>支持跨设备秒级同步</span>
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

      {/* Send Files Modal */}
      {selectedDeviceForSend && (
        <SendModal
          targetDevice={selectedDeviceForSend}
          isOpen={true}
          onClose={() => setSelectedDeviceForSend(null)}
          onSuccess={handleTransferSent}
        />
      )}
    </div>
  );
};
