import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { invoke, invokeResults, resetTauriMock } from "../test/tauri-mock";

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { SettingsModal } from "./SettingsModal";
import { AppSettings } from "../types";

const baseSettings: AppSettings = {
  server_url: "wss://example.com:58921",
  account_id: "alice",
  psk_secret: "secret",
  auto_inject: false,
  rate_limit_mb: 10,
  start_minimized: false,
  history_max_entries: 100,
  transfer_card_retain_secs: 30,
};

function setup(settings: Partial<AppSettings> = {}) {
  invokeResults["cmd_get_autostart"] = false;
  const onSave = vi.fn().mockResolvedValue(undefined);
  render(
    <SettingsModal
      settings={{ ...baseSettings, ...settings }}
      isOpen
      onClose={vi.fn()}
      onSave={onSave}
    />
  );
  return { onSave, user: userEvent.setup() };
}

const retainInput = () => screen.getByLabelText("完成任务在界面保持秒数");
const historyInput = () => screen.getByLabelText("传输历史保留条数");
const submit = () => screen.getByRole("button", { name: /保存配置/ });

describe("设置面板的数值校验", () => {
  beforeEach(() => {
    resetTauriMock();
  });

  it("合法输入正常提交，数值转成 number 交给后端", async () => {
    const { onSave, user } = setup();

    await user.clear(historyInput());
    await user.type(historyInput(), "50");
    await user.clear(retainInput());
    await user.type(retainInput(), "15");
    await user.click(submit());

    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({
      history_max_entries: 50,
      transfer_card_retain_secs: 15,
    });
  });

  it("0 是合法值（不限制 / 不自动消失），不被当成空输入", async () => {
    const { onSave, user } = setup();

    await user.clear(historyInput());
    await user.type(historyInput(), "0");
    await user.clear(retainInput());
    await user.type(retainInput(), "0");
    await user.click(submit());

    expect(onSave.mock.calls[0][0]).toMatchObject({
      history_max_entries: 0,
      transfer_card_retain_secs: 0,
    });
  });

  it("空输入被拦下且不提交——Number('') === 0 会把空值静默变成「不限制」", async () => {
    const { onSave, user } = setup();

    await user.clear(historyInput());
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/不能为空/)).toBeInTheDocument();
  });

  it("保持秒数超过 86400 被拦下——再大就会撞上 setTimeout 的 32 位上限", async () => {
    const { onSave, user } = setup();

    await user.clear(retainInput());
    await user.type(retainInput(), "99999999");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/超出上限 86400/)).toBeInTheDocument();
  });

  it("保留条数不受 86400 限制，它不喂给 setTimeout", async () => {
    const { onSave, user } = setup();

    await user.clear(historyInput());
    await user.type(historyInput(), "100000");
    await user.click(submit());

    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({ history_max_entries: 100000 });
  });

  it("超出 u32 上限的保留条数被拦下", async () => {
    const { onSave, user } = setup();

    await user.clear(historyInput());
    await user.type(historyInput(), "99999999999");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/超出上限 4294967295/)).toBeInTheDocument();
  });

  it("小数被拦下——后端 u32 反序列化会整单失败且只返回笼统错误", async () => {
    const { onSave, user } = setup();

    // number input 会吞掉直接键入的小数点，这里直接改 value 模拟粘贴
    await user.clear(retainInput());
    await user.type(retainInput(), "2.5");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
  });
});
