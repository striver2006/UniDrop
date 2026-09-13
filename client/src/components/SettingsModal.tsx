import React, { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { X, Save, Shield, AlertCircle, Server as ServerIcon } from "lucide-react";
import { AppSettings, ServerLimits } from "../types";

/**
 * 把一项限额格式化成可读文案。
 *
 * `0` 必须显示成「不限制」而不是「0 B」。两者语义正好相反——用户看到 0
 * 会读成「配额为零、什么都发不了」，而它实际表示该项限制已被关闭。
 */
const formatLimitBytes = (n: number): string => {
  if (!n || n <= 0) return "不限制";
  const KB = 1024;
  if (n < KB) return `${n} B`;
  if (n < KB * KB) return `${(n / KB).toFixed(1)} KB`;
  if (n < KB * KB * KB) return `${(n / (KB * KB)).toFixed(1)} MB`;
  return `${(n / (KB * KB * KB)).toFixed(2)} GB`;
};

const formatLimitCount = (n: number): string => (!n || n <= 0 ? "不限制" : `${n}`);

const LimitRow: React.FC<{ label: string; value: string }> = ({ label, value }) => (
  <div className="flex justify-between">
    <dt className="text-slate-500">{label}</dt>
    <dd className="text-slate-300 tabular-nums">{value}</dd>
  </div>
);

/** Rust 侧两个字段都是 u32，超出这个范围反序列化会整单失败 */
const U32_MAX = 4294967295;

/**
 * 保持秒数的业务上限：24 小时。
 *
 * 不只是「合理范围」的问题——setTimeout 的延迟以 32 位有符号整数存储，上限
 * 2147483647 ms（约 24.86 天）。秒数再大，secs * 1000 就会溢出并被降级成 1ms，
 * 卡片在出现的瞬间消失：用户想要「留久一点」，得到的却是「立刻消失」，
 * 语义完全反转且没有任何报错。
 */
const MAX_RETAIN_SECS = 86400;

// 注意：数字输入一律**不设** min / max / step 属性，边界只由 parseU32 把关。
// 这些属性会触发浏览器原生表单校验，在 submit 事件之前就拦下提交，于是
// handleSubmit 根本不跑：用户看到的是原生气泡（样式不可控、文案非中文），
// 而填字母时看到的却是我们的红字提示——同一个输入框两套错误呈现。
// step 默认就是 1，去掉它不影响上下箭头的步进。

/// 把输入框里的字符串解析成 u32。空串、负数、小数、越界一律判错。
///
/// `min` 默认 0。**空串提示必须随 min 变化**：固定写「不限制请填 0」的话，
/// 用在 min=1 的清理间隔上就成了主动引导用户填一个非法值。
const parseU32 = (
  raw: string,
  max: number = U32_MAX,
  min: number = 0
): { value: number } | { error: string } => {
  const trimmed = raw.trim();
  if (trimmed === "") {
    return { error: min > 0 ? `不能为空（最小 ${min}）` : "不能为空（不限制请填 0）" };
  }
  // 只认纯数字，顺带排除了 "-1"、"2.5"、"1e3" 与 Number("") === 0 这个坑
  if (!/^\d+$/.test(trimmed)) return { error: `只能填 ${min} 到 ${max} 的整数` };
  const value = Number(trimmed);
  if (!Number.isSafeInteger(value) || value > max) {
    return { error: `数值超出上限 ${max}` };
  }
  if (value < min) {
    return { error: `不能小于 ${min}` };
  }
  return { value };
};

/// 校验账号标识。规则必须与服务端 `auth/identity.go` 一致：
/// 1..=64 字节的 `[A-Za-z0-9._@-]`。
///
/// 形态对齐 parseU32：返回 `{value}` 或 `{error}`，由 handleSubmit 汇总进
/// fieldErrors，用同一套红字渲染。
const parseAccountId = (raw: string): { value: string } | { error: string } => {
  const trimmed = raw.trim();
  if (trimmed === "") return { error: "不能为空" };
  if (trimmed.length > 64) return { error: "长度不能超过 64" };
  if (!/^[A-Za-z0-9._@-]+$/.test(trimmed)) {
    return { error: "只能包含字母、数字与 . _ @ -" };
  }
  return { value: trimmed };
};

/** 缓存保留小时数上限：1 年 */
const MAX_CACHE_TTL_HOURS = 8760;
/** 缓存容量上限：1 TB（MB 计） */
const MAX_CACHE_SIZE_MB = 1048576;
/** 清理间隔上限 1 天、下限 1 分钟。下限不可为 0：调度循环拿到零间隔会退化成忙等。 */
const MAX_SWEEP_INTERVAL_MINUTES = 1440;
const MIN_SWEEP_INTERVAL_MINUTES = 1;

interface SettingsModalProps {
  settings: AppSettings;
  isOpen: boolean;
  onClose: () => void;
  onSave: (newSettings: AppSettings) => void | Promise<void>;
  /**
   * 服务端下发的限额，只读展示。
   *
   * `null` = 尚未拿到（未连接，或对端是不下发这个字段的老服务端）。
   * 此时显示「未下发」，**不要显示 0，也不要拿默认值顶上**——显示 0 会被读成
   * 配额为零，显示默认值会让用户以为那就是实际生效的值。
   */
  serverLimits?: ServerLimits | null;
}

export const SettingsModal: React.FC<SettingsModalProps> = ({
  settings,
  isOpen,
  onClose,
  onSave,
  serverLimits = null,
}) => {
  const [form, setForm] = useState<AppSettings>(settings);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 两个数字字段用字符串暂存输入：直接绑 number 的话，用户清空重填的中间态会
  // 变成 0，而 0 在这里是**有意义的取值**（不限制 / 不自动消失），不能与空输入混淆。
  const [historyLimitInput, setHistoryLimitInput] = useState(String(settings.history_max_entries));
  const [retainSecsInput, setRetainSecsInput] = useState(String(settings.transfer_card_retain_secs));
  const [cacheTtlInput, setCacheTtlInput] = useState(String(settings.cache_ttl_hours));
  const [cacheSizeInput, setCacheSizeInput] = useState(String(settings.cache_max_size_mb));
  const [sweepIntervalInput, setSweepIntervalInput] = useState(
    String(settings.cache_sweep_interval_minutes)
  );
  const [fieldErrors, setFieldErrors] = useState<{
    account?: string;
    history?: string;
    retain?: string;
    ttl?: string;
    size?: string;
    interval?: string;
  }>({});

  // 开机自启不属于 AppSettings：它的事实源是操作系统，本地不留副本，
  // 因此独立拉取、独立写入，与表单保存的失败域互不污染。
  const [autostart, setAutostart] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);

  useEffect(() => {
    setForm(settings);
    setError(null);
    setHistoryLimitInput(String(settings.history_max_entries));
    setRetainSecsInput(String(settings.transfer_card_retain_secs));
    setCacheTtlInput(String(settings.cache_ttl_hours));
    setCacheSizeInput(String(settings.cache_max_size_mb));
    setSweepIntervalInput(String(settings.cache_sweep_interval_minutes));
    setFieldErrors({});
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

    // 先做字段级校验再提交。后端 u32 反序列化失败只会返回笼统的「保存设置失败」，
    // 用户看不出是哪个字段的问题，所以小数、负数、超上限都必须在这里拦住。
    const account = parseAccountId(form.account_id);
    const history = parseU32(historyLimitInput);
    // 保持秒数额外受 setTimeout 的 32 位上限约束，不能放行到 u32::MAX
    const retain = parseU32(retainSecsInput, MAX_RETAIN_SECS);
    const ttl = parseU32(cacheTtlInput, MAX_CACHE_TTL_HOURS);
    const size = parseU32(cacheSizeInput, MAX_CACHE_SIZE_MB);
    // 间隔下限 1：零间隔会让后台清理循环退化成忙等
    const interval = parseU32(
      sweepIntervalInput,
      MAX_SWEEP_INTERVAL_MINUTES,
      MIN_SWEEP_INTERVAL_MINUTES
    );
    const nextFieldErrors = {
      account: "error" in account ? account.error : undefined,
      history: "error" in history ? history.error : undefined,
      retain: "error" in retain ? retain.error : undefined,
      ttl: "error" in ttl ? ttl.error : undefined,
      size: "error" in size ? size.error : undefined,
      interval: "error" in interval ? interval.error : undefined,
    };
    if (Object.values(nextFieldErrors).some(Boolean)) {
      setFieldErrors(nextFieldErrors);
      return;
    }
    setFieldErrors({});

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
        account_id: (account as { value: string }).value,
        psk_secret: form.psk_secret.trim(),
        history_max_entries: (history as { value: number }).value,
        transfer_card_retain_secs: (retain as { value: number }).value,
        cache_ttl_hours: (ttl as { value: number }).value,
        cache_max_size_mb: (size as { value: number }).value,
        cache_sweep_interval_minutes: (interval as { value: number }).value,
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
      <div className="bg-slate-900 border border-slate-700 rounded-2xl w-full max-w-sm max-h-full flex flex-col shadow-2xl">
        <div className="flex items-center justify-between px-5 pt-5 pb-3 border-b border-slate-800 shrink-0">
          <div className="flex items-center space-x-2">
            <Shield className="w-5 h-5 text-teal-400" />
            <h3 className="font-semibold text-slate-100">连接与安全偏好</h3>
          </div>
          <button onClick={onClose} className="text-slate-400 hover:text-slate-200">
            <X className="w-5 h-5" />
          </button>
        </div>

        <form onSubmit={handleSubmit} className="flex flex-col min-h-0 flex-1 text-xs">
          {/* 字段区独立滚动：窗口固定 380x560 且不可缩放，设置项一多就会把
              底部按钮顶出可视区，导致弹窗既看不全也关不掉。 */}
          <div className="flex-1 overflow-y-auto px-5 pt-4 space-y-3.5">
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
            <label htmlFor="account-id" className="block text-slate-400 mb-1">
              账号标识 (Account ID)
            </label>
            {/* 这里刻意**不加** required，与上面的 server_url 不同。
                理由同数字输入框那条注释：原生校验在 submit 之前拦下，handleSubmit
                根本不跑，用户看到的是样式不可控的英文原生气泡，而填非法字符时
                看到的却是我们的中文红字——同一个输入框两套错误呈现。
                更要命的是 required 判定 "   " 为已填写，而提交前的 trim 会把它
                变成空串，于是一个原生校验放行的值恰好是服务端必然拒绝的值。
                server_url / psk_secret 的 required 本轮不动，这处不一致是有意的。 */}
            <input
              id="account-id"
              type="text"
              value={form.account_id}
              onChange={(e) => setForm({ ...form, account_id: e.target.value })}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            {fieldErrors.account && (
              <p className="mt-1 text-[11px] text-rose-400 flex items-start gap-1">
                <span>{fieldErrors.account}</span>
              </p>
            )}
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

          {/* 文案要说清后果，而不是只说「允许自签证书」：用户据此做的是
              一个安全取舍，不告诉他代价就等于替他做了决定。
              三类场景都要列全——少写「非公共 CA」那一类，用内部 CA 的用户会
              以为这个开关与自己无关，最后仍然被引导来勾选它。 */}
          <div className="flex items-center justify-between pt-2">
            <div className="pr-3">
              <span className="font-medium text-slate-200">允许不安全连接</span>
              <p className="text-[11px] text-slate-500">
                跳过服务器证书校验，用于自签证书、IP 直连或非公共 CA 签发的部署
              </p>
              {form.allow_insecure_tls && (
                <p className="mt-1 text-[11px] text-amber-400">
                  已关闭证书校验：任何中间人都可冒充服务器读取你传输的内容
                </p>
              )}
            </div>
            <input
              type="checkbox"
              checked={form.allow_insecure_tls}
              onChange={(e) => setForm({ ...form, allow_insecure_tls: e.target.checked })}
              className="w-4 h-4 rounded text-teal-500 focus:ring-teal-400 bg-slate-800 border-slate-700"
            />
          </div>

          {/* 与上面的 TLS 开关相对：那个默认关闭（安全值是 false），
              这个默认开启（安全值是 true）。文案同样要说清它保证什么、
              不保证什么——「端到端加密」四个字很容易被读成比实际更强的承诺。 */}
          <div className="flex items-center justify-between pt-2">
            <div className="pr-3">
              <span className="font-medium text-slate-200">端到端加密</span>
              <p className="text-[11px] text-slate-500">
                内容在本机加密后才经中继转发，服务器看不到明文；对端版本过旧时会回落为不加密并提示
              </p>
              {!form.e2ee_enabled && (
                <p className="mt-1 text-[11px] text-amber-400">
                  已关闭：传输内容将以明文经过中继服务器
                </p>
              )}
            </div>
            <input
              type="checkbox"
              // 旁边的说明文字在独立的 div 里，与 input 没有 label 关联，
              // 屏幕阅读器与测试都定位不到。新加的开关补上 aria-label；
              // 上面几个既有开关同样缺，但那属于另一件事，不在本轮一并动。
              aria-label="端到端加密"
              checked={form.e2ee_enabled}
              onChange={(e) => setForm({ ...form, e2ee_enabled: e.target.checked })}
              className="w-4 h-4 rounded text-teal-500 focus:ring-teal-400 bg-slate-800 border-slate-700"
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

          <div className="pt-2">
            <label htmlFor="history-max-entries" className="block text-slate-400 mb-1">
              传输历史保留条数
            </label>
            <input
              id="history-max-entries"
              type="number"
              value={historyLimitInput}
              onChange={(e) => setHistoryLimitInput(e.target.value)}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            <p className="text-[11px] text-slate-500 mt-1">
              超出的最旧记录连同缓存文件一起删除；填 0 表示不限制
            </p>
            {fieldErrors.history && (
              <div className="flex items-start space-x-1.5 text-[11px] text-rose-400 mt-1">
                <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
                <span>{fieldErrors.history}</span>
              </div>
            )}
          </div>

          <div>
            <label htmlFor="transfer-card-retain-secs" className="block text-slate-400 mb-1">
              完成任务在界面保持秒数
            </label>
            <input
              id="transfer-card-retain-secs"
              type="number"
              value={retainSecsInput}
              onChange={(e) => setRetainSecsInput(e.target.value)}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            <p className="text-[11px] text-slate-500 mt-1">
              传输完成的卡片到点自动消失；填 0 表示不自动消失，最大 {MAX_RETAIN_SECS} 秒（24 小时）。
              失败的卡片不受影响，始终保留到手动关闭
            </p>
            {fieldErrors.retain && (
              <div className="flex items-start space-x-1.5 text-[11px] text-rose-400 mt-1">
                <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
                <span>{fieldErrors.retain}</span>
              </div>
            )}
          </div>


          <div className="pt-1 border-t border-slate-800/60">
            <p className="text-[11px] font-medium text-slate-300 pt-2">磁盘缓存清理</p>
          </div>

          <div>
            <label htmlFor="cache-ttl-hours" className="block text-slate-400 mb-1">
              缓存保留小时数
            </label>
            <input
              id="cache-ttl-hours"
              type="number"
              value={cacheTtlInput}
              onChange={(e) => setCacheTtlInput(e.target.value)}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            <p className="text-[11px] text-slate-500 mt-1">
              超过该时长的缓存文件会被清理；填 0 表示不按时间清理，最大 {MAX_CACHE_TTL_HOURS} 小时（1 年）
            </p>
            {fieldErrors.ttl && (
              <div className="flex items-start space-x-1.5 text-[11px] text-rose-400 mt-1">
                <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
                <span>{fieldErrors.ttl}</span>
              </div>
            )}
          </div>

          <div>
            <label htmlFor="cache-max-size-mb" className="block text-slate-400 mb-1">
              缓存容量上限 (MB)
            </label>
            <input
              id="cache-max-size-mb"
              type="number"
              value={cacheSizeInput}
              onChange={(e) => setCacheSizeInput(e.target.value)}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            <p className="text-[11px] text-slate-500 mt-1">
              超出后按最近最少使用清理到上限的 80%；填 0 表示不限容量。10240 MB = 10 GB
            </p>
            {fieldErrors.size && (
              <div className="flex items-start space-x-1.5 text-[11px] text-rose-400 mt-1">
                <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
                <span>{fieldErrors.size}</span>
              </div>
            )}
          </div>

          <div>
            <label htmlFor="cache-sweep-interval" className="block text-slate-400 mb-1">
              清理间隔 (分钟)
            </label>
            <input
              id="cache-sweep-interval"
              type="number"
              value={sweepIntervalInput}
              onChange={(e) => setSweepIntervalInput(e.target.value)}
              className="w-full bg-slate-800 border border-slate-700 rounded-lg px-3 py-2 text-slate-200 focus:outline-none focus:border-teal-500"
            />
            <p className="text-[11px] text-slate-500 mt-1">
              后台多久清理一次，最小 1 分钟。修改后在下一轮生效，重启应用立即生效
            </p>
            {fieldErrors.interval && (
              <div className="flex items-start space-x-1.5 text-[11px] text-rose-400 mt-1">
                <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
                <span>{fieldErrors.interval}</span>
              </div>
            )}
          </div>

          {/* 服务端限额：纯展示，本轮没有任何输入框。
              限额的事实源在服务端环境变量里，客户端只能看。不写明这一点的话，
              用户会对着一组灰数字猜为什么改不了。 */}
          <div className="pt-1">
            <div className="flex items-center space-x-1.5 text-slate-300 mb-1">
              <ServerIcon className="w-3.5 h-3.5 text-slate-500" />
              <span className="font-medium">服务端限额</span>
            </div>
            <p className="text-[11px] text-slate-500 mb-2">
              由服务端配置，客户端只读。如需调整请联系部署者
            </p>
            {serverLimits === null ? (
              <div className="text-[11px] text-slate-500 bg-slate-800/50 border border-slate-800 rounded-lg px-3 py-2">
                未下发（尚未连接，或服务端版本较旧）
              </div>
            ) : (
              <dl className="text-[11px] bg-slate-800/50 border border-slate-800 rounded-lg px-3 py-2 space-y-1">
                <LimitRow label="单个文件" value={formatLimitBytes(serverLimits.max_single_file_bytes)} />
                <LimitRow label="单次总量" value={formatLimitBytes(serverLimits.max_total_transfer_bytes)} />
                <LimitRow label="剪贴板图片" value={formatLimitBytes(serverLimits.max_clipboard_image_bytes)} />
                <LimitRow label="剪贴板文本" value={formatLimitBytes(serverLimits.max_clipboard_text_bytes)} />
                <LimitRow label="单次文件数" value={formatLimitCount(serverLimits.max_items_per_offer)} />
                <LimitRow label="同时传输数" value={formatLimitCount(serverLimits.max_concurrent_transfers)} />
              </dl>
            )}
          </div>
          </div>

          <div className="shrink-0 px-5 py-4 border-t border-slate-800 space-y-3">
          {error && (
            <div className="flex items-start space-x-1.5 text-[11px] text-rose-400">
              <AlertCircle className="w-3.5 h-3.5 mt-px shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <div className="flex space-x-2">
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
          </div>
        </form>
      </div>
    </div>
  );
};
