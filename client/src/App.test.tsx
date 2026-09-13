import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";

import {
  invoke,
  listen,
  emitEvent,
  invokeResults,
  listenerCount,
  resetTauriMock,
} from "./test/tauri-mock";

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({ hide: vi.fn() }),
}));

import { App } from "./App";
import { AppSettings, ActiveTransfer } from "./types";

const baseSettings: AppSettings = {
  server_url: "wss://example.com:58921",
  account_id: "alice",
  psk_secret: "s",
  auto_inject: false,
  start_minimized: false,
  history_max_entries: 100,
  transfer_card_retain_secs: 30,
  cache_ttl_hours: 24,
  cache_max_size_mb: 10240,
  cache_sweep_interval_minutes: 60,
  allow_insecure_tls: false,
};

function transfer(overrides: Partial<ActiveTransfer> = {}): ActiveTransfer {
  return {
    session_id: "s1",
    preview_summary: "demo.txt",
    total_size: 100,
    transferred_size: 100,
    direction: "RECEIVE",
    progress: 100,
    status: "COMPLETED",
    data_type: "FILES",
    ...overrides,
  };
}

/** 渲染 App 并等初始数据拉取落地 */
async function renderApp(settings: Partial<AppSettings> = {}) {
  invokeResults["cmd_get_self_info"] = {
    device_id: "d1",
    hostname: "mac",
    os_type: "macos",
    app_version: "0.1.1",
  };
  invokeResults["cmd_get_online_devices"] = [];
  invokeResults["cmd_get_settings"] = { ...baseSettings, ...settings };
  invokeResults["cmd_list_history"] = [];
  // 默认按「老服务端」处理：不下发限额。需要限额的用例自行覆盖。
  invokeResults["cmd_get_server_limits"] = null;

  const utils = render(<App />);
  // fetchInitialData 的四个 await 需要让出 microtask 队列
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
  return utils;
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("完成卡片自动消失（需求 5）", () => {
  beforeEach(() => {
    resetTauriMock();
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("COMPLETED 卡片在设定秒数后自动消失", async () => {
    await renderApp({ transfer_card_retain_secs: 30 });

    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });
    expect(screen.getByText("demo.txt")).toBeInTheDocument();

    // 差一点点不能消失，否则说明延迟算错了
    await advance(29_000);
    expect(screen.getByText("demo.txt")).toBeInTheDocument();

    await advance(1_500);
    expect(screen.queryByText("demo.txt")).not.toBeInTheDocument();
  });

  it("FAILED 卡片永不自动消失——失败原因的入口不能被定时器吃掉", async () => {
    await renderApp({ transfer_card_retain_secs: 30 });

    await act(async () => {
      emitEvent("transfer-progress", transfer({ status: "FAILED" }));
    });

    // 远超保持秒数
    await advance(300_000);
    expect(screen.getByText("demo.txt")).toBeInTheDocument();
  });

  it("保持秒数为 0 时不注册定时器", async () => {
    await renderApp({ transfer_card_retain_secs: 0 });

    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });

    await advance(600_000);
    expect(screen.getByText("demo.txt")).toBeInTheDocument();
  });

  it("重复到达的终态事件不会堆积多个定时器", async () => {
    await renderApp({ transfer_card_retain_secs: 10 });

    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });
    // 5 秒后同一会话再来一次 COMPLETED，定时器应被重置而不是叠加
    await advance(5_000);
    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });

    // 距第二次事件 9 秒（距第一次 14 秒）：若旧定时器没被清掉，这里已经消失了
    await advance(9_000);
    expect(screen.getByText("demo.txt")).toBeInTheDocument();

    await advance(1_500);
    expect(screen.queryByText("demo.txt")).not.toBeInTheDocument();
  });

  it("秒数极大时截断到 setTimeout 上限，不因 32 位溢出而当场闪退", async () => {
    // 86400 是表单上限；这里直接喂一个会溢出的值，验证 App 侧的兜底截断。
    // 未截断的话 secs * 1000 溢出后被降级为 1ms，卡片瞬间消失。
    await renderApp({ transfer_card_retain_secs: 4_000_000 });

    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });

    await advance(5_000);
    expect(
      screen.getByText("demo.txt"),
      "卡片不应在几秒内消失——那正是溢出降级为 1ms 的症状"
    ).toBeInTheDocument();
  });

  it("卸载时清理定时器，不对已卸载组件 setState", async () => {
    const { unmount } = await renderApp({ transfer_card_retain_secs: 5 });

    await act(async () => {
      emitEvent("transfer-progress", transfer());
    });

    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    unmount();
    await advance(10_000);

    expect(errorSpy).not.toHaveBeenCalled();
    expect(listenerCount("transfer-progress")).toBe(0);
    errorSpy.mockRestore();
  });
});

describe("TLS 证书失败提示", () => {
  // 这条守的是「事件发了但没人接」这一整类缺陷。
  // 后端 connection_actor 在证书校验失败时 emit tls-cert-failed，
  // 若前端不监听，默认开启校验后自签/IP/内部 CA 三类部署升级即进入
  // 无提示的静默重连——而 USER_GUIDE 已经向用户许诺了这个提示。
  it("注册了 tls-cert-failed 监听", async () => {
    await renderApp();
    expect(listenerCount("tls-cert-failed")).toBe(1);
  });

  it("证书失败时显示常驻横幅，并引导去连接设置", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", "无法验证服务器证书：UnknownIssuer。请在「设置」中勾选「允许不安全连接」。");
    });

    expect(screen.getByText(/无法验证服务器证书/)).toBeInTheDocument();
    // 文案不得沿用鉴权失败那套：证书问题改密钥没有用
    expect(screen.queryByText(/鉴权失败/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查连接设置" })).toBeInTheDocument();
    expect(screen.getByText("证书不受信任")).toBeInTheDocument();
  });

  // 连接 actor 每次退避重试都会重发该事件，横幅必须幂等而不是越堆越多
  it("重复收到同一事件不会堆叠横幅", async () => {
    await renderApp();

    for (let i = 0; i < 3; i++) {
      await act(async () => {
        emitEvent("tls-cert-failed", "无法验证服务器证书：UnknownIssuer");
      });
    }

    expect(screen.getAllByText(/无法验证服务器证书/)).toHaveLength(1);
  });

  it("连接恢复后横幅消失", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", "无法验证服务器证书：UnknownIssuer");
    });
    expect(screen.getByText(/无法验证服务器证书/)).toBeInTheDocument();

    await act(async () => {
      emitEvent("devices-updated", []);
    });
    expect(screen.queryByText(/无法验证服务器证书/)).not.toBeInTheDocument();
  });
});

describe("切账号清空传输卡片", () => {
  // 历史面板靠重拉，传输卡片没有「重拉」可言——它是推送累积的本地状态，
  // 只能清空。不清的话上一个账号的完成卡片会继续挂在界面中央，
  // 上面的「装载到剪贴板」按钮点下去必然撞上后端闸门报错，
  // 而卡片本身就带着上一个账号的文件名与摘要。
  it("收到 account-changed 后清空卡片", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("transfer-progress", transfer({ session_id: "s1", status: "COMPLETED" }));
    });
    expect(screen.getByText("demo.txt")).toBeInTheDocument();

    await act(async () => {
      emitEvent("account-changed", undefined);
    });

    expect(screen.queryByText("demo.txt")).not.toBeInTheDocument();
  });
});
