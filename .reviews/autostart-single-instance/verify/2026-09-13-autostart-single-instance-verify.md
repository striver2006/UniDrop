# 编译测试记录 · autostart-single-instance

- **运行**：`20260912T174039Z`
- **结论**：✅ 通过
- **起止**：2026-09-12T17:40:39.087Z → 2026-09-12T17:40:41.676Z
- **项目根**：`/Users/chenzhenbo/Work/UniClip`

## 一、执行的命令

| # | 组 | 命令 | 退出码 | 耗时 | 命令 sha256 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| 1 | build | `cd client && pnpm build` | 0 | 1.5s | `ed1e6660f646` |
| 2 | build | `cd server && go build ./...` | 0 | 0.9s | `54ca0b984c1f` |
| 3 | test | `cd server && go test ./...` | 0 | 0.2s | `a40f6eeafb2b` |

> `命令 sha256` 取命令原文，用于与配置对账（AC-10）。

## 二、输出摘要

### `cd client && pnpm build`（build，退出码 0）

**stdout（尾部）**

```text

> unidrop-client-ui@0.1.1 build /Users/chenzhenbo/Work/UniClip/client
> tsc && vite build

vite v5.4.21 building for production...
transforming...
✓ 1577 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                   0.47 kB │ gzip:  0.31 kB
dist/assets/index-tUCprSPS.css   18.58 kB │ gzip:  4.11 kB
dist/assets/index-CkmCBf7A.js   204.87 kB │ gzip: 60.85 kB
✓ built in 709ms
```

### `cd server && go build ./...`（build，退出码 0）

（无输出）

### `cd server && go test ./...`（test，退出码 0）

**stdout（尾部）**

```text
?   	github.com/unidrop/unidrop-server/cmd/diag-receiver	[no test files]
?   	github.com/unidrop/unidrop-server/cmd/diag-sender	[no test files]
?   	github.com/unidrop/unidrop-server/cmd/unidrop-server	[no test files]
ok  	github.com/unidrop/unidrop-server/internal	(cached)
ok  	github.com/unidrop/unidrop-server/internal/auth	(cached)
?   	github.com/unidrop/unidrop-server/internal/config	[no test files]
ok  	github.com/unidrop/unidrop-server/internal/controller	(cached)
ok  	github.com/unidrop/unidrop-server/internal/protocol	(cached)
ok  	github.com/unidrop/unidrop-server/internal/registry	(cached)
ok  	github.com/unidrop/unidrop-server/internal/relay	(cached)
?   	github.com/unidrop/unidrop-server/internal/stun	[no test files]
```

