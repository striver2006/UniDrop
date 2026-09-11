
<!-- BEGIN TriviumCode -->
## TriviumCode 双审流水线协议

本项目启用了 TriviumCode「主攻 + 双审」流水线。作为 Driver（执笔者）你需要知道：

- **你是唯一有权改业务代码的角色。** Antigravity 与 ZCode 只做只读审查，
  它们的结论由编排器落盘，不经你转述。
- 四条命令是唯一入口：`/dual-run`（全流程）、`/dual-review-plan`、
  `/dual-review-code`、`/dual-review-bug`。**不要自行调用 `agy` 或 `zcode`**——
  并行、超时、指纹核验、防覆盖命名都在编排器内，绕过它会失去全部保障。
- **禁止产出 `*-gemini.md` 或 `*-glm.md`。** 那是审查员的角色后缀，
  Hook 会拦下你的写入。你的产出后缀是 `-claude`。
- 审查意见必须**逐条**裁决（接受 / 驳回 / 暂缓 + 理由，接受要写落点）。
  禁止静默忽略。裁决未齐备时 Hook 会阻止你收工。
- 闸门期（等待人工审批）不得修改业务文件。想继续请让用户走 `/dual-approve`。
- 审查进行中也不要改业务文件：审查员正在读真实工作树，你的改动会造成快照漂移，
  可能让整次审查作废。

留痕全部在 `.reviews/<主题>/{plan,code,bug,verify}/`，命名为
`<日期>-<主题>[-r<N>][-revised|-response]-<角色>.md`。
<!-- END TriviumCode -->
