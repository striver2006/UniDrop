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
  rate_limit_mb: 10,
  start_minimized: false,
  history_max_entries: 100,
  transfer_card_retain_secs: 30,
  cache_ttl_hours: 24,
  cache_max_size_mb: 10240,
  cache_sweep_interval_minutes: 60,
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

  const utils = render(<App />);
  // fetchInitialData 的三个 await 需要让出 microtask 队列
  await act(async () => {
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
