import React, { useState, useEffect, useRef } from "react";
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
import { OnlineDevice, AppSettings, ActiveTransfer , ServerLimits } from "./types";
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
  start_minimized: false,
  // 与 Rust 侧 AppSettings::default_config() 必须保持一致（两份独立字面量）
  history_max_entries: 100,
  transfer_card_retain_secs: 30,
  cache_ttl_hours: 24,
  cache_max_size_mb: 10240,
  cache_sweep_interval_minutes: 60,
  allow_insecure_tls: false,
};

export const App: React.FC = () => {
  const [selfDevice, setSelfDevice] = useState<OnlineDevice | null>(null);
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [transfers, setTransfers] = useState<ActiveTransfer[]>([]);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [selectedDeviceForSend, setSelectedDeviceForSend] = useState<OnlineDevice | null>(null);
  /// 连接层错误横幅。kind 决定文案与引导按钮：
  /// "auth" 是 PSK / 账号格式被服务端拒绝，"tls" 是服务器证书校验失败。
  /// 两者的修复动作不同——前者改密钥或账号，后者要么换受信任的证书、
  /// 要么显式勾选「允许不安全连接」——所以不能共用一句「检查密钥」。
  const [connError, setConnError] = useState<{ kind: "auth" | "tls"; message: string } | null>(null);
  // 服务端下发的限额。null = 尚未拿到（未连接，或老服务端不发这个字段）。
  // 不要用默认值顶上——那会让用户以为看到的就是实际生效的值。
  const [serverLimits, setServerLimits] = useState<ServerLimits | null>(null);
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

  // 下面那个监听用的 useEffect 依赖数组是 []，它的闭包永远看到**首次渲染**的
  // settings，也就是 defaultSettings——真实配置要等 fetchInitialData 拉回来才有。
  // 所以定时器秒数必须经 ref 读取，直接读 settings 会永远拿到默认值。
  const settingsRef = useRef<AppSettings>(defaultSettings);
  useEffect(() => {
    settingsRef.current = settings;
  }, [settings]);

  // 按 session_id 记账：transfer-progress 对同一会话会反复到达，终态事件也可能
  // 重复送达，不记账就会给同一张卡片堆积多个定时器。
  const dismissTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(new Map());

  const clearDismissTimer = (sessionId: string) => {
    const timer = dismissTimersRef.current.get(sessionId);
    if (timer !== undefined) {
      clearTimeout(timer);
      dismissTimersRef.current.delete(sessionId);
    }
  };

  /// 只给**完成**的卡片排自动消失。秒数取事件到达时的配置，不追溯已在计时的卡片。
  const scheduleAutoDismiss = (sessionId: string) => {
    clearDismissTimer(sessionId);
    const secs = settingsRef.current.transfer_card_retain_secs;
    if (!secs || secs <= 0) return; // 0 = 不自动消失
    // setTimeout 的延迟以 32 位有符号整数存储，超过 2147483647ms 会溢出并被降级
    // 成 1ms——卡片当场闪退，恰好是这个设置项语义的反面。SettingsModal 已把上限
    // 卡在 86400 秒，这里再截断一次作兜底，防止日后有人绕开表单写入更大的值。
    const delayMs = Math.min(secs * 1000, 2147483647);
    const timer = setTimeout(() => {
      dismissTimersRef.current.delete(sessionId);
      setTransfers((prev) => prev.filter((t) => t.session_id !== sessionId));
    }, delayMs);
    dismissTimersRef.current.set(sessionId, timer);
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

      // 事件是推送来的，打开设置面板时可能早已错过，所以这里主动补拉一次。
      const limits = await invoke<ServerLimits | null>("cmd_get_server_limits");
      setServerLimits(limits);
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
      setConnError(null); // Devices updated implies successful authentication
    });

    // 2. Listen for auth events
    const unlistenAuthPromise = listen<string>("auth-failed", (event) => {
      setConnError({ kind: "auth", message: event.payload });
      showNotification(`身份验证失败: ${event.payload}`, "error");
    });

    const unlistenAuthSuccessPromise = listen("auth-success", () => {
      setConnError(null);
      showNotification("安全鉴权成功，已连接中继服务器", "success");
    });

    // TLS 证书校验失败。
    //
    // 刻意**不弹 toast**：连接 actor 每次退避重试（≤30s）都会重新 emit，
    // 弹 toast 会堆成一串。常驻横幅天然幂等——重复 set 同一内容不产生新 UI。
    const unlistenTlsPromise = listen<string>("tls-cert-failed", (event) => {
      setConnError({ kind: "tls", message: event.payload });
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
        scheduleAutoDismiss(update.session_id);
      } else if (update.status === "FAILED") {
        showNotification(`传输失败: ${update.preview_summary}`, "error");
        // 失败卡片**不**自动消失：错误 toast 只显示 4 秒，卡片再自动消失之后
        // 就没有任何醒目入口能看到这次为什么失败了。只能由用户手动点 X 移除。
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

    const unlistenLimitsPromise = listen<ServerLimits | null>("server-limits-updated", (event) => {
      setServerLimits(event.payload ?? null);
    });

    return () => {
      unlistenLimitsPromise.then((unlisten) => unlisten());
      unlistenDevicesPromise.then((unlisten) => unlisten());
      unlistenAuthPromise.then((unlisten) => unlisten());
      unlistenAuthSuccessPromise.then((unlisten) => unlisten());
      unlistenTlsPromise.then((unlisten) => unlisten());
      unlistenProgressPromise.then((unlisten) => unlisten());
      unlistenOfferPromise.then((unlisten) => unlisten());
      unlistenSettingsPromise.then((unlisten) => unlisten());
      // 卸载后定时器若仍触发就会对已卸载的组件 setState。StrictMode 下开发期
      // 会 mount→unmount→mount，不清会稳定复现重复定时器。
      dismissTimersRef.current.forEach((timer) => clearTimeout(timer));
      dismissTimersRef.current.clear();
    };
  }, []);

  const handleSaveSettings = async (newSettings: AppSettings) => {
    try {
      await invoke("cmd_save_settings", { newSettings });
      setSettings(newSettings);
      setConnError(null);
      showNotification("配置已保存并即时生效，正在重连中继服务器...", "success");
      setTimeout(() => {
        fetchInitialData();
      }, 300);
    } catch (err: any) {
      console.error("save settings error:", err);
      showNotification(typeof err === "string" ? err : "保存设置失败", "error");
      // 向上抛出，让设置弹窗保持打开、保留用户已填内容
      throw err;
    }
  };

  const handleSendToDevice = (target: OnlineDevice) => {
    setSelectedDeviceForSend(target);
  };

  const handleTransferSent = (_sessionId: string, count: number) => {
    showNotification(`已发起发送请求 (${count} 个文件)，等待对方接收...`, "info");
  };

  const handleDismissTransfer = (sessionId: string) => {
    // 同时清掉定时器，否则它会留到超时才空转一次，期间还持有已移除会话的闭包
    clearDismissTimer(sessionId);
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
              {/* 版本取后端上报值（编译期来自 Cargo.toml），不要写死：
                  写死的那份不会随发版更新，迟早和设备列表里显示的对不上 */}
              {selfDevice && (
                <span className="text-[10px] px-1.5 py-0.5 rounded-full bg-teal-500/20 text-teal-300 font-normal">
                  v{selfDevice.app_version}
                </span>
              )}
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

      {/* Connection Error Banner */}
      {connError && (
        <div className="bg-rose-950/80 border-b border-rose-800/80 px-4 py-2 flex items-center justify-between text-xs text-rose-300">
          <div className="flex items-center space-x-2 truncate">
            <ShieldAlert className="w-4 h-4 text-rose-400 shrink-0" />
            <span className="truncate">
              {connError.kind === "tls" ? connError.message : `鉴权失败: ${connError.message}`}
            </span>
          </div>
          <button
            onClick={() => setIsSettingsOpen(true)}
            className="text-[11px] underline hover:text-rose-200 font-medium shrink-0 ml-2"
          >
            {connError.kind === "tls" ? "检查连接设置" : "检查密钥"}
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
        <div className={`flex items-center space-x-1 ${connError ? "text-rose-400" : "text-teal-500/90"}`}>
          {connError ? <ShieldAlert className="w-3.5 h-3.5" /> : <ShieldCheck className="w-3.5 h-3.5" />}
          <span>
            {connError ? (connError.kind === "tls" ? "证书不受信任" : "连接未授权") : "PSK 接入安全就绪"}
          </span>
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
          serverLimits={serverLimits}
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
