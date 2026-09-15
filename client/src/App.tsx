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
import {
  OnlineDevice,
  AppSettings,
  ActiveTransfer,
  ServerLimits,
  TlsCertFailure,
  TrayPlacementPayload,
  certFailureStatusText,
} from "./types";
import { MenuBarHiddenBanner } from "./components/MenuBarHiddenBanner";
import { NotificationPermBanner } from "./components/NotificationPermBanner";
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
  tls_trust_mode: "public_ca",
  pinned_cert_sha256: [],
  e2ee_enabled: true,
};

export const App: React.FC = () => {
  const [selfDevice, setSelfDevice] = useState<OnlineDevice | null>(null);
  const [devices, setDevices] = useState<OnlineDevice[]>([]);
  const [transfers, setTransfers] = useState<ActiveTransfer[]>([]);
  const [settings, setSettings] = useState<AppSettings>(defaultSettings);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [selectedDeviceForSend, setSelectedDeviceForSend] = useState<OnlineDevice | null>(null);
  /// macOS 26 菜单栏放置状态。null = 本平台无此机制（Windows / Linux）。
  /// 「推」（事件）与「拉」（命令）两条通道都要有：开了 start_minimized 时
  /// 窗口是隐藏的，后端探测很可能早于这里注册 listener，只推不拉横幅永远不出现。
  const [trayPlacement, setTrayPlacement] = useState<TrayPlacementPayload | null>(null);
  /// 系统通知授权状态。null = 本平台无此机制或尚未探测。
  /// 与 trayPlacement 同一套「推 + 拉」双通道：后端在 setup 里就完成首次探测，
  /// 那时 listener 大概率还没注册上，只推不拉横幅永远不出现。
  const [notifAuthGranted, setNotifAuthGranted] = useState<boolean | null>(null);
  /// 连接层错误横幅。kind 决定文案与引导按钮：
  /// "auth" 是 PSK / 账号格式被服务端拒绝，"tls" 是服务器证书校验失败。
  /// 两者的修复动作不同——前者改密钥或账号，后者按证书失败的**具体档位**而定——
  /// 所以不能共用一句「检查密钥」。
  ///
  /// tls 一支带的是整个结构化的分类结果而不是一句拼好的话：证书失败有好几种，
  /// 处置互不相同（改地址 / 续期 / 重签 / 换 CA），拼字符串会逼着它们共用一句建议。
  const [connError, setConnError] = useState<
    { kind: "auth"; message: string } | { kind: "tls"; failure: TlsCertFailure } | null
  >(null);
  /// 证书错误详情是否展开。
  ///
  /// **必须独立于 connError，且只在 connError 清空时复位。** 连接 actor 每次
  /// 退避重试（≤30s）都会重发 tls-cert-failed，若跟着事件一起复位，
  /// 用户刚展开的详情会每隔几秒自己合上——而那个现象看起来像渲染 bug，
  /// 没人会想到是重连在后面推事件。
  const [certDetailExpanded, setCertDetailExpanded] = useState(false);
  // 服务端下发的限额。null = 尚未拿到（未连接，或老服务端不发这个字段）。
  // 不要用默认值顶上——那会让用户以为看到的就是实际生效的值。
  const [serverLimits, setServerLimits] = useState<ServerLimits | null>(null);
  const [notification, setNotification] = useState<{
    type: "info" | "success" | "error";
    text: string;
  } | null>(null);

  // 横幅消失时才收起详情。
  //
  // 放在 effect 里而不是跟着三处 setConnError(null) 各写一遍，是为了让
  // 「展开状态只由横幅的生死决定」这件事只有一个落点——将来再多一处清空点，
  // 也不会漏掉复位。反过来也重要：**不能**在收到 tls-cert-failed 时复位，
  // 那个事件每次重连都会重发，会把用户刚展开的详情反复合上。
  useEffect(() => {
    if (!connError) setCertDetailExpanded(false);
  }, [connError]);

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

      // 托盘放置状态单独 try：它只是一条提示，失败绝不该把整个初始化
      // 拖进上面那个 catch，让用户看到一句「拉取初始状态失败」的红字。
      try {
        const tp = await invoke<TrayPlacementPayload | null>("cmd_get_tray_placement");
        setTrayPlacement(tp);
      } catch (e) {
        console.warn("tray placement probe unavailable:", e);
      }

      // 通知授权状态同理单独 try，理由同上。
      try {
        const granted = await invoke<boolean | null>("cmd_get_notification_auth_status");
        setNotifAuthGranted(granted);
      } catch (e) {
        console.warn("notification auth probe unavailable:", e);
      }
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

    // 切账号：清空主界面的传输卡片。
    //
    // 历史面板那边靠重拉（HistoryPanel 自己监听同一事件），但传输卡片是
    // 推送累积出来的本地状态，没有「重拉」可言，只能清空。
    // 不清的话，上一个账号刚完成的接收卡片会继续挂在界面中央，上面的
    // 「装载到剪贴板」按钮点下去必然撞上后端闸门报错——而且卡片本身就带着
    // 上一个账号的文件名与摘要。`transfer_card_retain_secs` 设为 0 时它永不消失。
    //
    // 同时要清掉自动消失的定时器：卡片没了还留着 timer，等它触发时会对着
    // 一个已经不存在的 session 调 setState。
    const unlistenAccountPromise = listen("account-changed", () => {
      dismissTimersRef.current.forEach((timer) => clearTimeout(timer));
      dismissTimersRef.current.clear();
      setTransfers([]);
    });

    // TLS 证书校验失败。
    //
    // 刻意**不弹 toast**：连接 actor 每次退避重试（≤30s）都会重新 emit，
    // 弹 toast 会堆成一串。常驻横幅天然幂等——重复 set 同一内容不产生新 UI。
    const unlistenTlsPromise = listen<TlsCertFailure>("tls-cert-failed", (event) => {
      setConnError({ kind: "tls", failure: event.payload });
    });

    // E2EE 回落：本次传输没能加密。
    //
    // 用 toast 而不是常驻横幅，与 tls-cert-failed 相反：那个是持续性的连接故障、
    // 每次重试都会重发，横幅才幂等；这个是**一次传输一条**的事件，
    // 用横幅反而会一直挂着，让人以为当前状态仍然不安全。
    // 这里可以直接调 showNotification（不必像 settingsRef 那样过 ref）：
    // 它只依赖 setNotification，而 React 保证 setState 的引用稳定，
    // 不读任何会过期的 state。
    const unlistenE2eeFallbackPromise = listen<string>("e2ee-fallback", (event) => {
      showNotification(event.payload, "error");
    });

    // 收到一份解不开的加密 OFFER，已拒收。最常见的原因是两端 PSK 不一致。
    const unlistenE2eeRejectPromise = listen<string>("e2ee-offer-rejected", (event) => {
      showNotification(event.payload, "error");
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

    // macOS 菜单栏放置状态变化（被拒 / 已恢复）。看门狗在恢复时也会推一次，
    // 横幅据此自行消失，用户不需要重启应用。
    const unlistenTrayPlacementPromise = listen<TrayPlacementPayload>(
      "macos-tray-placement",
      (event) => {
        setTrayPlacement(event.payload);
      }
    );

    // 通知授权状态变化（启动探测 / 投递失败时后端都会推）。授权在系统侧被
    // 翻转（重装、改系统设置）不需要重启应用，横幅要能跟着事件出现/消失。
    const unlistenNotifAuthPromise = listen<{ granted: boolean }>(
      "notification-auth-status",
      (event) => {
        setNotifAuthGranted(event.payload.granted);
      }
    );

    const unlistenLimitsPromise = listen<ServerLimits | null>("server-limits-updated", (event) => {
      setServerLimits(event.payload ?? null);
    });

    return () => {
      unlistenLimitsPromise.then((unlisten) => unlisten());
      unlistenDevicesPromise.then((unlisten) => unlisten());
      unlistenAuthPromise.then((unlisten) => unlisten());
      unlistenAuthSuccessPromise.then((unlisten) => unlisten());
      unlistenTlsPromise.then((unlisten) => unlisten());
      unlistenE2eeFallbackPromise.then((unlisten) => unlisten());
      unlistenE2eeRejectPromise.then((unlisten) => unlisten());
      unlistenAccountPromise.then((unlisten) => unlisten());
      unlistenProgressPromise.then((unlisten) => unlisten());
      unlistenOfferPromise.then((unlisten) => unlisten());
      unlistenSettingsPromise.then((unlisten) => unlisten());
      unlistenTrayPlacementPromise.then((unlisten) => unlisten());
      unlistenNotifAuthPromise.then((unlisten) => unlisten());
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

      {/*
        菜单栏放置横幅排在连接错误之上：它讲的是「你为什么找不到这个应用」，
        比「连不上服务器」更靠前一层——后者至少还能看到窗口。
      */}
      <MenuBarHiddenBanner
        payload={trayPlacement}
        onOpenSettings={async () => {
          try {
            await invoke("cmd_open_menu_bar_settings");
          } catch (e) {
            // URL scheme 未文档化，打不开是可能的。横幅里的文字路径始终可见，
            // 所以这里只提示一句，不必把失败演成故障。
            showNotification("请手动打开：系统设置 → 菜单栏", "error");
          }
        }}
        onDismiss={async () => {
          try {
            await invoke("cmd_dismiss_tray_guidance");
            setTrayPlacement((prev) => (prev ? { ...prev, force_reveal_optout: true } : prev));
          } catch (e) {
            console.warn("failed to persist tray guidance opt-out:", e);
          }
        }}
      />

      {/* 通知授权横幅排在菜单栏横幅之下：它讲的是「收到东西却没人告诉你」，
          比连接错误更值得先看见——内容其实已经收下来了。 */}
      <NotificationPermBanner
        granted={notifAuthGranted}
        onOpenSettings={async () => {
          try {
            await invoke("cmd_open_notification_settings");
          } catch (e) {
            // URL scheme 未文档化，打不开是可能的。横幅里的文字路径始终可见，
            // 所以这里只提示一句，不必把失败演成故障（同菜单栏横幅的口径）。
            showNotification("请手动打开：系统设置 → 通知", "error");
          }
        }}
      />

      {/* Connection Error Banner */}
      {connError && (
        <div className="bg-rose-950/80 border-b border-rose-800/80 px-4 py-2 text-xs text-rose-300">
          <div className="flex items-start justify-between gap-2">
            {/*
              min-w-0 是这里的关键，不是随手加的：flex 子项的 min-width 默认是 auto，
              不加它，无论去掉 truncate 还是换成 break-words 都不会换行，
              只会变成横向溢出。改造前这里是 `flex items-center` + `truncate`，
              两个问题叠在一起，结果把后端拼好的多行处置建议整段吃掉——
              用户只看到「无法验证服务器证书：IO error: invalid peer ...」，
              而省略号后面那个词恰恰是唯一能决定怎么修的信息。
            */}
            <div className="flex items-start gap-2 min-w-0">
              <ShieldAlert className="w-4 h-4 text-rose-400 shrink-0 mt-0.5" />
              <div className="min-w-0">
                <p className="font-medium break-words">
                  {connError.kind === "tls"
                    ? connError.failure.title
                    : `鉴权失败: ${connError.message}`}
                </p>
                {connError.kind === "tls" && certDetailExpanded && (
                  <div className="mt-1.5 space-y-1.5 text-rose-300/90">
                    {/*
                      窗口只有约 760px 宽、高度也有限，而通配符证书的 SAN 可能几十条。
                      不封顶会把下面的设备列表整个挤出视口。
                    */}
                    <ul className="space-y-0.5 max-h-32 overflow-y-auto">
                      {connError.failure.detail.map((line, i) => (
                        <li key={i} className="break-words">
                          {line}
                        </li>
                      ))}
                    </ul>
                    {connError.failure.observed_cert_sha256 && (
                      /*
                        实际看到的证书指纹。
                        刻意**不给**「信任这张证书」的一键按钮：错误横幅下方的
                        一键信任，恰恰是训练用户对中间人警告无脑点确认的经典形态，
                        而这一整轮改造的目的就是拆掉这类捷径。这里只给出指纹和
                        核对方法，多出来的那点摩擦正是它的价值——它逼用户至少
                        看一眼那串 hex，也留出了带外核对的时机。
                      */
                      <div className="pt-1 border-t border-rose-800/50">
                        <p className="text-[10px] text-rose-300/80">客户端实际看到的证书指纹：</p>
                        <div className="flex items-start gap-2 mt-0.5">
                          <code className="text-[10px] font-mono break-all text-rose-200/90 flex-1 min-w-0">
                            {connError.failure.observed_cert_sha256}
                          </code>
                          <button
                            type="button"
                            onClick={() =>
                              navigator.clipboard
                                ?.writeText(connError.failure.observed_cert_sha256 ?? "")
                                .catch(() => {})
                            }
                            className="text-[10px] underline hover:text-rose-200 shrink-0"
                          >
                            复制
                          </button>
                        </div>
                        <p className="text-[10px] text-rose-300/70 mt-0.5">
                          请先在服务器上执行 openssl x509 -fingerprint -sha256 -noout -in
                          &lt;证书文件&gt; 核对一致，再填入设置
                        </p>
                      </div>
                    )}
                    {/* 分类错了的时候，原始错误是唯一的现场，必须能看到。
                        但它同样嵌着对端可控的 SAN 文本，所以和上面的 detail 一样
                        要封高度——后端已做字符清洗与长度截断，这里是第二道。 */}
                    <p className="text-[10px] text-rose-400/70 break-all font-mono max-h-20 overflow-y-auto">
                      {connError.failure.raw}
                    </p>
                  </div>
                )}
              </div>
            </div>
            <div className="shrink-0 flex items-center gap-3">
              {connError.kind === "tls" && (
                <button
                  onClick={() => setCertDetailExpanded((v) => !v)}
                  className="text-[11px] underline hover:text-rose-200 font-medium"
                >
                  {certDetailExpanded ? "收起" : "详情"}
                </button>
              )}
              <button
                onClick={() => setIsSettingsOpen(true)}
                className="text-[11px] underline hover:text-rose-200 font-medium"
              >
                {connError.kind === "tls" ? "检查连接设置" : "检查密钥"}
              </button>
            </div>
          </div>
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
            {connError
              ? connError.kind === "tls"
                // 不再一律说「证书不受信任」：名字不匹配那一档里证书**是**受信任的，
                // 说成不受信任会把用户推去关校验，而正确动作是改地址。
                ? certFailureStatusText(connError.failure.kind)
                : "连接未授权"
              : "PSK 接入安全就绪"}
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
