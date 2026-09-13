import { vi } from "vitest";

/**
 * Tauri 事件与命令的测试替身。
 *
 * 组件在 useEffect 里调 `listen(name, cb)`，注册是**同步**发生的（返回的 Promise
 * 只用来拿 unlisten），所以 render 之后立刻 `emitEvent` 就能命中回调，
 * 不需要等待 Promise。
 */
type Handler = (event: { payload: unknown }) => void;

const listeners = new Map<string, Set<Handler>>();

/** 记录每个命令被调用的次数与参数，断言「是否重拉了历史」这类行为要用 */
export const invokeCalls: Array<{ cmd: string; args?: Record<string, unknown> }> = [];

/** 命令返回值表，测试可按需覆盖 */
export const invokeResults: Record<string, unknown> = {};

export const listen = vi.fn(async (name: string, cb: Handler) => {
  if (!listeners.has(name)) listeners.set(name, new Set());
  listeners.get(name)!.add(cb);
  return () => {
    listeners.get(name)?.delete(cb);
  };
});

export const invoke = vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
  invokeCalls.push({ cmd, args });
  if (cmd in invokeResults) return invokeResults[cmd];
  throw new Error(`未预设返回值的命令: ${cmd}`);
});

/** 模拟后端广播一个事件 */
export function emitEvent(name: string, payload: unknown) {
  listeners.get(name)?.forEach((cb) => cb({ payload }));
}

/** 某个事件当前的监听者数量——用来断言卸载时确实解绑了 */
export function listenerCount(name: string) {
  return listeners.get(name)?.size ?? 0;
}

export function countInvokes(cmd: string) {
  return invokeCalls.filter((c) => c.cmd === cmd).length;
}

export function resetTauriMock() {
  listeners.clear();
  invokeCalls.length = 0;
  for (const k of Object.keys(invokeResults)) delete invokeResults[k];
  listen.mockClear();
  invoke.mockClear();
}
