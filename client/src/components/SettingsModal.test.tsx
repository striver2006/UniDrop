import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import { invoke, invokeResults, resetTauriMock } from "../test/tauri-mock";

vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { SettingsModal } from "./SettingsModal";
import { AppSettings, ServerLimits } from "../types";

const baseSettings: AppSettings = {
  server_url: "wss://example.com:58921",
  account_id: "alice",
  psk_secret: "secret",
  auto_inject: false,
  start_minimized: false,
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

function setup(
  settings: Partial<AppSettings> = {},
  serverLimits: ServerLimits | null = null
) {
  invokeResults["cmd_get_autostart"] = false;
  const onSave = vi.fn().mockResolvedValue(undefined);
  render(
    <SettingsModal
      settings={{ ...baseSettings, ...settings }}
      isOpen
      onClose={vi.fn()}
      onSave={onSave}
      serverLimits={serverLimits}
    />
  );
  return { onSave, user: userEvent.setup() };
}

const limitsFixture: ServerLimits = {
  max_single_file_bytes: 128 * 1024 * 1024,
  max_total_transfer_bytes: 256 * 1024 * 1024,
  max_clipboard_image_bytes: 64 * 1024 * 1024,
  max_clipboard_text_bytes: 4 * 1024 * 1024,
  max_items_per_offer: 64,
  max_concurrent_transfers: 8,
};

const retainInput = () => screen.getByLabelText("完成任务在界面保持秒数");
const ttlInput = () => screen.getByLabelText("缓存保留小时数");
const sizeInput = () => screen.getByLabelText("缓存容量上限 (MB)");
const intervalInput = () => screen.getByLabelText("清理间隔 (分钟)");
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

  // ---------- 磁盘缓存清理（需求 8） ----------

  it("三个缓存字段正常提交", async () => {
    const { onSave, user } = setup();

    await user.clear(ttlInput());
    await user.type(ttlInput(), "48");
    await user.clear(sizeInput());
    await user.type(sizeInput(), "2048");
    await user.clear(intervalInput());
    await user.type(intervalInput(), "15");
    await user.click(submit());

    expect(onSave).toHaveBeenCalledTimes(1);
    expect(onSave.mock.calls[0][0]).toMatchObject({
      cache_ttl_hours: 48,
      cache_max_size_mb: 2048,
      cache_sweep_interval_minutes: 15,
    });
  });

  it("TTL 与容量的 0 是合法值（关闭该段清理）", async () => {
    const { onSave, user } = setup();

    await user.clear(ttlInput());
    await user.type(ttlInput(), "0");
    await user.clear(sizeInput());
    await user.type(sizeInput(), "0");
    await user.click(submit());

    expect(onSave.mock.calls[0][0]).toMatchObject({
      cache_ttl_hours: 0,
      cache_max_size_mb: 0,
    });
  });

  /// 间隔是三个字段里唯一不能为 0 的：零间隔会让后台清理循环退化成忙等。
  it("清理间隔为 0 被拦下——它不能像另两个字段那样表示「关闭」", async () => {
    const { onSave, user } = setup();

    await user.clear(intervalInput());
    await user.type(intervalInput(), "0");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/不能小于 1/)).toBeInTheDocument();
  });

  it("间隔字段清空时的提示不会引导用户填 0", async () => {
    const { onSave, user } = setup();

    await user.clear(intervalInput());
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/不能为空（最小 1）/)).toBeInTheDocument();
    // 这条提示只属于 min>0 的字段，不能是那句「不限制请填 0」
    expect(screen.queryByText(/不限制请填 0/)).not.toBeInTheDocument();
  });

  it("容量上限超过 1TB 被拦下", async () => {
    const { onSave, user } = setup();

    await user.clear(sizeInput());
    await user.type(sizeInput(), "2000000");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/超出上限 1048576/)).toBeInTheDocument();
  });

  it("保留小时数超过 1 年被拦下", async () => {
    const { onSave, user } = setup();

    await user.clear(ttlInput());
    await user.type(ttlInput(), "99999");
    await user.click(submit());

    expect(onSave).not.toHaveBeenCalled();
    expect(screen.getByText(/超出上限 8760/)).toBeInTheDocument();
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

// ---------- 服务端限额只读展示（需求 3） ----------

describe("服务端限额只读展示（需求 3）", () => {
  beforeEach(() => resetTauriMock());

  it("未下发时显示「未下发」，而不是 0，也不是默认值", () => {
    setup({}, null);

    expect(screen.getByText(/未下发/)).toBeInTheDocument();
    // 不得拿默认值顶上——那会让用户以为这就是实际生效的值
    expect(screen.queryByText("128.0 MB")).not.toBeInTheDocument();
  });

  it("下发后按可读单位展示各项", () => {
    setup({}, limitsFixture);

    expect(screen.getByText("128.0 MB")).toBeInTheDocument();
    expect(screen.getByText("256.0 MB")).toBeInTheDocument();
    expect(screen.getByText("64.0 MB")).toBeInTheDocument();
    expect(screen.getByText("4.0 MB")).toBeInTheDocument();
    expect(screen.getByText("64")).toBeInTheDocument();
  });

  it("某项为 0 时显示「不限制」——显示 0 会被读成配额为零，语义正好相反", () => {
    setup({}, { ...limitsFixture, max_single_file_bytes: 0, max_items_per_offer: 0 });

    expect(screen.getAllByText("不限制")).toHaveLength(2);
    expect(screen.queryByText("0 B")).not.toBeInTheDocument();
  });

  it("只读分区不产生可提交字段，保存时不污染 payload", async () => {
    const { onSave, user } = setup({}, limitsFixture);

    await user.click(submit());

    expect(onSave).toHaveBeenCalledTimes(1);
    const submitted = onSave.mock.calls[0][0];
    expect(submitted).not.toHaveProperty("max_single_file_bytes");
    expect(submitted).not.toHaveProperty("rate_limit_mb");
  });
});

describe("账号标识校验", () => {
  const accountInput = () => screen.getByLabelText("账号标识 (Account ID)");

  it("空账号被拦下且不提交", async () => {
    const { onSave, user } = setup();
    await user.clear(accountInput());
    await user.click(submit());

    expect(await screen.findByText("不能为空")).toBeInTheDocument();
    expect(onSave).not.toHaveBeenCalled();
  });

  // 原生 required 判定纯空格为「已填写」，而提交前的 trim 会把它变成空串 ——
  // 一个原生校验放行的值恰好是服务端必然拒绝的值。这条钉死我们自己拦住它。
  it("纯空格账号被拦下", async () => {
    const { onSave, user } = setup();
    await user.clear(accountInput());
    await user.type(accountInput(), "   ");
    await user.click(submit());

    expect(await screen.findByText("不能为空")).toBeInTheDocument();
    expect(onSave).not.toHaveBeenCalled();
  });

  it("含空格或斜杠的账号被拦下", async () => {
    const { onSave, user } = setup();
    for (const bad of ["a b", "acct/other"]) {
      await user.clear(accountInput());
      await user.type(accountInput(), bad);
      await user.click(submit());
      expect(await screen.findByText("只能包含字母、数字与 . _ @ -")).toBeInTheDocument();
    }
    expect(onSave).not.toHaveBeenCalled();
  });

  it("超长账号被拦下", async () => {
    const { onSave, user } = setup();
    await user.clear(accountInput());
    await user.type(accountInput(), "a".repeat(65));
    await user.click(submit());

    expect(await screen.findByText("长度不能超过 64")).toBeInTheDocument();
    expect(onSave).not.toHaveBeenCalled();
  });

  // 正例：防止规则被收紧成拒绝一切
  it("合法账号正常提交，且被 trim", async () => {
    const { onSave, user } = setup();
    await user.clear(accountInput());
    await user.type(accountInput(), "  my_team_sync  ");
    await user.click(submit());

    await waitFor(() => expect(onSave).toHaveBeenCalled());
    expect(onSave.mock.calls[0][0]).toMatchObject({ account_id: "my_team_sync" });
  });

  it("端到端加密开关默认开启，关闭时给出明文警告", async () => {
    const { user } = setup();

    const toggle = screen.getByRole("checkbox", { name: /端到端加密/ });
    expect(toggle).toBeChecked();
    // 开启时不该出现警告——警告只用于「你以为安全但其实不是」的状态
    expect(screen.queryByText(/将以明文经过中继服务器/)).not.toBeInTheDocument();

    await user.click(toggle);
    expect(screen.getByText(/将以明文经过中继服务器/)).toBeInTheDocument();
  });

  describe("连接安全三档", () => {
    const modeRadio = (name: RegExp) => screen.getByRole("radio", { name });

    it("默认选中「校验公共 CA 证书」，且不显示任何警告", () => {
      setup();
      expect(modeRadio(/校验公共 CA 证书/)).toBeChecked();
      expect(screen.queryByText(/已关闭证书校验/)).not.toBeInTheDocument();
      // 指纹输入框只在中间档出现
      expect(screen.queryByLabelText("证书 SHA-256 指纹")).not.toBeInTheDocument();
    });

    it("选到「允许不安全连接」才出现中间人警告", async () => {
      const { user } = setup();
      await user.click(modeRadio(/允许不安全连接/));
      expect(screen.getByText(/任何中间人都可冒充服务器/)).toBeInTheDocument();
    });

    it("选到「信任指定证书」时出现指纹输入框与域名提醒", async () => {
      const { user } = setup();
      await user.click(modeRadio(/信任指定证书/));

      expect(screen.getByLabelText("证书 SHA-256 指纹")).toBeInTheDocument();
      // 这一条是该档放弃掉的保证，不写出来用户无从判断风险
      expect(screen.getByText(/不再检查域名与有效期/)).toBeInTheDocument();
    });

    it("中间档不填指纹时拒绝保存", async () => {
      const { onSave, user } = setup();
      await user.click(modeRadio(/信任指定证书/));
      await user.click(submit());

      expect(await screen.findByText(/至少要填一条证书指纹/)).toBeInTheDocument();
      expect(onSave).not.toHaveBeenCalled();
    });

    it("指纹格式错误时拒绝保存并指出是哪一条", async () => {
      const { onSave, user } = setup();
      await user.click(modeRadio(/信任指定证书/));
      await user.type(screen.getByLabelText("证书 SHA-256 指纹"), "not-a-fingerprint");
      await user.click(submit());

      expect(await screen.findByText(/指纹格式不对/)).toBeInTheDocument();
      expect(onSave).not.toHaveBeenCalled();
    });

    it("接受 openssl 原样输出，并把档位与老字段一起提交", async () => {
      const { onSave, user } = setup();
      const hexed = "ab".repeat(32);
      const colonized = hexed.toUpperCase().match(/.{2}/g)!.join(":");

      await user.click(modeRadio(/信任指定证书/));
      await user.type(
        screen.getByLabelText("证书 SHA-256 指纹"),
        `SHA256 Fingerprint=${colonized}`
      );
      await user.click(submit());

      await waitFor(() => expect(onSave).toHaveBeenCalled());
      const saved = onSave.mock.calls[0][0];
      expect(saved.tls_trust_mode).toBe("pinned");
      // 老字段必须跟着走，否则降级回旧版本会读出错误的信任状态
      expect(saved.allow_insecure_tls).toBe(false);
      expect(saved.pinned_cert_sha256).toHaveLength(1);
    });

    it("切到不校验档时老字段同步为 true", async () => {
      const { onSave, user } = setup();
      await user.click(modeRadio(/允许不安全连接/));
      await user.click(submit());

      await waitFor(() => expect(onSave).toHaveBeenCalled());
      expect(onSave.mock.calls[0][0]).toMatchObject({
        tls_trust_mode: "insecure",
        allow_insecure_tls: true,
      });
    });
  });
});
