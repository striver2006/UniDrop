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
  tls_trust_mode: "public_ca",
  pinned_cert_sha256: [],
  e2ee_enabled: true,
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
  // 与 Rust 侧 core::tls_trust::TlsCertFailure 同形。
  // 事故现场那一档单独造一个，因为它恰恰是「证书完全合法、错的是地址」
  // 这种最容易被提示误导的情形。
  const nameMismatchFailure = {
    kind: "name_mismatch" as const,
    expected: "120.26.54.84",
    presented: ["www.leafun.xyz"],
    title: "服务器证书有效，但不是签发给 120.26.54.84 的",
    // 与后端 describe() 的实际产出保持一致：带外核对排在改地址之前。
    // 顺序本身是有意义的——用户读到前半句就会动手，核对要求放后面等于没写。
    detail: [
      "证书签发给：www.leafun.xyz",
      "你连接的是：120.26.54.84",
      "先确认上面列出的域名确实是你自己的服务器——证书由公共 CA 签发只说明对方拥有那个域名，不说明那台机器是你的。",
      "确认无误后，把「服务器地址」改为证书上的域名，端口保持不变。",
      "若这个域名你并不认识，不要改地址，也不要关闭证书校验——那可能意味着连接被中间人接管了。",
    ],
    raw: 'IO error: invalid peer certificate: certificate not valid for name "120.26.54.84"; certificate is only valid for www.leafun.xyz',
    observed_cert_sha256: null,
  };

  const unknownIssuerFailure = {
    kind: "unknown_issuer" as const,
    title: "服务器证书由未知的签发者签发",
    detail: ["公共根证书库里没有这个签发者，因此无法确认对端身份。"],
    raw: "IO error: invalid peer certificate: UnknownIssuer",
    observed_cert_sha256: null,
  };

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
      emitEvent("tls-cert-failed", unknownIssuerFailure);
    });

    expect(screen.getByText(/未知的签发者/)).toBeInTheDocument();
    // 文案不得沿用鉴权失败那套：证书问题改密钥没有用
    expect(screen.queryByText(/鉴权失败/)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "检查连接设置" })).toBeInTheDocument();
    expect(screen.getByText("证书签发者未知")).toBeInTheDocument();
  });

  // 本次事故的回归测试。证书是合法的 Let's Encrypt 证书，只是签发给了域名
  // 而用户填的是 IP——此时「证书不受信任」是假话，而引导用户关校验
  // 等于为了一张完全合法的证书废掉整条链路的身份验证。
  it("名称不匹配时不说「证书不受信任」，也不建议关闭校验", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", nameMismatchFailure);
    });

    expect(screen.getByText("证书域名不匹配")).toBeInTheDocument();
    expect(screen.queryByText("证书不受信任")).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "详情" }).click();
    });
    expect(screen.queryByText(/允许不安全连接/)).not.toBeInTheDocument();
    // 带外核对要求必须真的渲染出来——它是这条指引不沦为攻击面的前提
    expect(screen.getByText(/确认上面列出的域名确实是你自己的服务器/)).toBeInTheDocument();
    expect(screen.getByText(/可能意味着连接被中间人接管/)).toBeInTheDocument();
  });

  // 截断是这次用户只看到 "invalid peer ..." 的直接原因：
  // 后端拼好的多行处置建议一个字都没露出来。
  it("展开详情后逐行显示完整处置建议，不被截断", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", nameMismatchFailure);
    });

    // 收起时只有标题
    expect(screen.queryByText(nameMismatchFailure.detail[2])).not.toBeInTheDocument();

    await act(async () => {
      screen.getByRole("button", { name: "详情" }).click();
    });

    for (const line of nameMismatchFailure.detail) {
      const el = screen.getByText(line);
      expect(el).toBeInTheDocument();
      // truncate 会把多行建议压成一行再截断，这正是要防的
      expect(el.className).not.toContain("truncate");
    }
    // 分类错了的时候原始错误是唯一的现场，必须也能看到
    expect(screen.getByText(nameMismatchFailure.raw)).toBeInTheDocument();
  });

  // 重连每 ≤30s 重发一次事件。跟着事件复位展开状态的话，
  // 用户刚点开的详情会自己合上，而那看起来像渲染 bug。
  it("重发事件不会把已展开的详情合上", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", nameMismatchFailure);
    });
    await act(async () => {
      screen.getByRole("button", { name: "详情" }).click();
    });
    expect(screen.getByText(nameMismatchFailure.detail[0])).toBeInTheDocument();

    for (let i = 0; i < 3; i++) {
      await act(async () => {
        emitEvent("tls-cert-failed", nameMismatchFailure);
      });
    }

    expect(screen.getByText(nameMismatchFailure.detail[0])).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "收起" })).toBeInTheDocument();
  });

  // 不给「信任这张证书」一键按钮是刻意的：警告下方的一键信任
  // 正是训练用户无脑点确认的经典形态。只给指纹，逼他至少看一眼。
  it("展示实际看到的证书指纹，但不提供一键信任", async () => {
    await renderApp();

    const fp = "ab".repeat(32);
    await act(async () => {
      emitEvent("tls-cert-failed", { ...unknownIssuerFailure, observed_cert_sha256: fp });
    });
    await act(async () => {
      screen.getByRole("button", { name: "详情" }).click();
    });

    expect(screen.getByText(fp)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "复制" })).toBeInTheDocument();
    expect(screen.getByText(/核对一致，再填入设置/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /信任这张证书/ })).not.toBeInTheDocument();
  });

  it("后端没给指纹时不显示指纹区块", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", unknownIssuerFailure);
    });
    await act(async () => {
      screen.getByRole("button", { name: "详情" }).click();
    });

    expect(screen.queryByRole("button", { name: "复制" })).not.toBeInTheDocument();
  });

  it("各档失败给出各自的状态栏文案", async () => {
    await renderApp();

    const cases: Array<[Record<string, unknown>, string]> = [
      [{ kind: "expired" }, "证书已过期"],
      [{ kind: "not_yet_valid" }, "证书尚未生效"],
      [{ kind: "revoked" }, "证书已被吊销"],
      [{ kind: "unsupported_version" }, "证书格式不受支持"],
      [{ kind: "other" }, "证书校验失败"],
    ];

    for (const [kind, expected] of cases) {
      await act(async () => {
        emitEvent("tls-cert-failed", {
          ...kind,
          title: "t",
          detail: ["d"],
          raw: "r",
          observed_cert_sha256: null,
        });
      });
      expect(screen.getByText(expected)).toBeInTheDocument();
    }
  });

  // 连接 actor 每次退避重试都会重发该事件，横幅必须幂等而不是越堆越多
  it("重复收到同一事件不会堆叠横幅", async () => {
    await renderApp();

    for (let i = 0; i < 3; i++) {
      await act(async () => {
        emitEvent("tls-cert-failed", unknownIssuerFailure);
      });
    }

    expect(screen.getAllByText(/未知的签发者/)).toHaveLength(1);
  });

  it("E2EE 回落时以 toast 提示，而不是常驻横幅", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("e2ee-fallback", "本次传输未加密：未能确认对端支持端到端加密");
    });

    expect(screen.getByText(/本次传输未加密/)).toBeInTheDocument();
    // 回落是「一次传输一条」的事件，不能走常驻横幅那条路——
    // 横幅会一直挂着，让人以为当前连接状态有问题。
    expect(screen.queryByRole("button", { name: "检查连接设置" })).not.toBeInTheDocument();
    expect(screen.queryByText(/鉴权失败/)).not.toBeInTheDocument();
  });

  it("收到解不开的加密 OFFER 时提示密钥可能不一致", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("e2ee-offer-rejected", "收到一份无法解密的传输请求，已拒收：两端密钥可能不一致");
    });

    expect(screen.getByText(/两端密钥可能不一致/)).toBeInTheDocument();
  });

  it("注册了两个 E2EE 事件监听", async () => {
    await renderApp();
    expect(listenerCount("e2ee-fallback")).toBe(1);
    expect(listenerCount("e2ee-offer-rejected")).toBe(1);
  });

  it("连接恢复后横幅消失", async () => {
    await renderApp();

    await act(async () => {
      emitEvent("tls-cert-failed", unknownIssuerFailure);
    });
    expect(screen.getByText(/未知的签发者/)).toBeInTheDocument();

    await act(async () => {
      emitEvent("devices-updated", []);
    });
    expect(screen.queryByText(/未知的签发者/)).not.toBeInTheDocument();
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
