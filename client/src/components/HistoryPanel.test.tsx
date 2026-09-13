import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, act, waitFor } from "@testing-library/react";

import {
  invoke,
  listen,
  emitEvent,
  invokeResults,
  countInvokes,
  listenerCount,
  resetTauriMock,
} from "../test/tauri-mock";

vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));

import { HistoryPanel } from "./HistoryPanel";
import { TransferHistoryEntry } from "../types";

function entry(session_id: string): TransferHistoryEntry {
  return {
    session_id,
    direction: "RECEIVE",
    data_type: "FILES",
    preview_summary: `文件-${session_id}`,
    total_size: 100,
    total_items: 1,
    status: "COMPLETED",
    error_message: null,
    created_at: "2026-09-13 00:00:00",
    completed_at: "2026-09-13 00:00:01",
    cached_count: 1,
  };
}

async function renderPanel(initial: TransferHistoryEntry[]) {
  invokeResults["cmd_list_history"] = initial;
  const utils = render(<HistoryPanel onNotify={vi.fn()} />);
  await waitFor(() => expect(countInvokes("cmd_list_history")).toBe(1));
  return utils;
}

describe("历史面板的刷新链路", () => {
  beforeEach(() => {
    resetTauriMock();
  });

  it("收到 history-pruned 后重新拉取历史——这是「保存设置后列表立刻变短」的唯一通路", async () => {
    await renderPanel([entry("s1"), entry("s2"), entry("s3")]);
    expect(screen.getByText("文件-s1")).toBeInTheDocument();

    // 后端修剪掉两条后广播
    invokeResults["cmd_list_history"] = [entry("s3")];
    await act(async () => {
      emitEvent("history-pruned", 2);
    });

    await waitFor(() => {
      expect(screen.queryByText("文件-s1")).not.toBeInTheDocument();
    });
    expect(screen.getByText("文件-s3")).toBeInTheDocument();
    expect(countInvokes("cmd_list_history")).toBe(2);
  });

  it("传输终态事件同样触发重拉（既有行为不被破坏）", async () => {
    await renderPanel([]);

    await act(async () => {
      emitEvent("transfer-progress", { session_id: "s1", status: "COMPLETED" });
    });

    await waitFor(() => expect(countInvokes("cmd_list_history")).toBe(2));
  });

  it("传输进行中不触发重拉——否则每个进度包都打一次数据库", async () => {
    await renderPanel([]);

    await act(async () => {
      emitEvent("transfer-progress", { session_id: "s1", status: "TRANSFERRING" });
    });

    expect(countInvokes("cmd_list_history")).toBe(1);
  });

  it("卸载时三个监听都解绑", async () => {
    const { unmount } = await renderPanel([]);
    expect(listenerCount("history-pruned")).toBe(1);
    expect(listenerCount("transfer-progress")).toBe(1);
    expect(listenerCount("account-changed")).toBe(1);

    unmount();

    await waitFor(() => {
      expect(listenerCount("history-pruned")).toBe(0);
      expect(listenerCount("transfer-progress")).toBe(0);
      expect(listenerCount("account-changed")).toBe(0);
    });
  });

  // 上一轮的 tls-cert-failed 就是「事件发了没人接」，这条防同类问题。
  // 监听必须落在 HistoryPanel——它才是真正拉 cmd_list_history 的地方；
  // 放在 App 里看着也通，但 fetchInitialData 根本不拉历史。
  it("注册了 account-changed 监听", async () => {
    await renderPanel([]);
    expect(listenerCount("account-changed")).toBe(1);
  });

  // 切账号后不重拉，列表会继续显示上一个账号的卡片，
  // 而卡片上的装载 / 另存为 / 定位三个按钮都会真的去读文件。
  // 不能指望 history-pruned 代劳：切账号时通常一条都不用修剪，那个事件不会来。
  it("收到 account-changed 后重新拉取历史", async () => {
    await renderPanel([entry("s1")]);
    const before = countInvokes("cmd_list_history");

    await act(async () => {
      emitEvent("account-changed", undefined);
    });

    await waitFor(() => {
      expect(countInvokes("cmd_list_history")).toBeGreaterThan(before);
    });
  });
});
