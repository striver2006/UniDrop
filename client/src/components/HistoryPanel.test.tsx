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

  it("卸载时两个监听都解绑", async () => {
    const { unmount } = await renderPanel([]);
    expect(listenerCount("history-pruned")).toBe(1);
    expect(listenerCount("transfer-progress")).toBe(1);

    unmount();

    await waitFor(() => {
      expect(listenerCount("history-pruned")).toBe(0);
      expect(listenerCount("transfer-progress")).toBe(0);
    });
  });
});
