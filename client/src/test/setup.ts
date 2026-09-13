import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// 每个用例后卸载组件：本目录里的测试大量使用假定时器，残留的已挂载组件
// 会在后续用例快进时间时触发它自己的定时器，造成难查的串扰。
afterEach(() => {
  cleanup();
});
