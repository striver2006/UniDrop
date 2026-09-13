---
schema: trivium.disposition.v1
topic: cache-cleanup-configurable
stage: code
role: claude
kind: response
run_id: 20260913T024250Z
addresses:
  - AGY-01
  - AGY-02
  - AGY-03
  - GLM-01
  - GLM-02
  - GLM-03
  - GLM-04
  - GLM-05
---

# 代码审查应答：磁盘缓存清理策略可配置化

- 主题：`cache-cleanup-configurable`
- 阶段：code（逐条应答与修改方案）
- 日期：2026-09-13
- 角色：Driver (claude)
- 被裁决的审查：`code/2026-09-13-cache-cleanup-configurable-gemini.md`、
  `code/2026-09-13-cache-cleanup-configurable-glm.md`

---

## 0. 应答摘要

**两侧判定不同**：Antigravity 判 `request-changes`（含 1 条 major），
ZCode 判 `approve-with-nits`（无 major）。差异全部来自 AGY-01 ——
ZCode 未发现那条过度删除。经独立核验（§1），**AGY-01 成立且确实是 major**，
因此本轮按 `request-changes` 处理，合入前必须先修。

共 8 条意见，**全部接受**，无驳回、无暂缓。收敛为 6 处：

1. **TTL 与容量同时启用时过度删除**（AGY-01，唯一 major）——
   计划阶段两侧都没预见，是实现期才产生的缺陷。
2. **三个新字段的 `serde(default)` 无测试守护**（AGY-02 + GLM-01，两侧独立命中）——
   我上一轮为 `history_max_entries` / `transfer_card_retain_secs` 专门写过这道闸，
   这轮加了三个同类字段却忘了扩展它。
3. **sweep 的装载竞态既无复核也无记录**（GLM-02）。
4. **`victims.contains` 在锁内 O(n²)**（GLM-03）——
   与本次重构自己宣称的理由相矛盾。
5. **sweep 的 `Err` 被静默吞掉**（GLM-04）。
6. **注释残留旧函数名**（AGY-03 + GLM-05，两侧独立命中）。

四个送审焦点（u64 换算、启动即扫、三段式锁纪律、常量收敛）两侧均核实通过。

---

## 1. AGY-01 的独立核验：两侧结论不一致，必须自己判

ZCode 通篇未提这条，Antigravity 判为 major。核验 `cache_manager.rs:184-236`：

```rust
if policy.ttl_enabled() {
    let expired = ...;          // 只 SELECT file_path，没有 size
    victims.extend(expired);
}
if policy.quota_enabled() {
    let total = SUM(file_size);  // 全部条目，未扣除 TTL 已选中的
    if total > max {
        let candidates = ... ORDER BY last_accessed_at ASC;
        let mut remaining = total;   // ← 从未扣除的总量开始
        for (path, size) in candidates {
            if remaining <= low { break; }
            remaining -= size;
            if !victims.contains(&path) { victims.push(path); }
        }
    }
}
```

**缺陷成立**。关键在于 `candidates` 的排序键是 `last_accessed_at`，
这与「是否过期」**毫无关系**。构造一个最小反例：

| 文件 | 是否过期 | `last_accessed_at` | 大小 |
|---|---|---|---|
| A | 否（活跃缓存） | 很旧 | 6 MB |
| B | 是（TTL 已选中） | 较新 | 6 MB |

上限 10 MB ⇒ 低水位 8 MB，总量 12 MB 超限。
仅删 B（TTL 本就要删）后剩 6 MB，早已低于水位，**A 不该被碰**。

但实际执行：`remaining = 12`，candidates 顺序是 A、B。
第一轮取 A —— `12 > 8` 不 break，`remaining = 6`，A 不在 victims ⇒ **A 被选中删除**。
一个未过期的活跃缓存就这样被误删了。

我那行注释「TTL 段已选中的不再重复计数，但它们的体积同样会被释放」正是这个 bug 的
根源：我当时只想到「循环里会扣减它们的体积」，却没意识到**扣减发生在遍历到它们的时候**，
而在此之前排在前面的非过期文件已经被选走了。

**既有测试为什么没发现**：`sweep_removes_files_past_ttl` 传 `policy(24, 0)`、
`sweep_enforces_quota_down_to_low_watermark` 传 `policy(0, 10)` —— 两条都把另一段
关成 0，**从未测过两段同时启用**。这是我测试设计的盲点，而默认配置（24 小时 + 10240 MB）
恰恰是两段都开的。

---

## 2. 逐条裁决

| 意见编号 | 来源 | 严重度 | 裁决 | 理由 | 落点 |
| :--- | :--- | :--- | :--- | :--- | :--- |
| AGY-01 | gemini | 重要 | 接受 | 已独立核验成立（§1 附最小反例）：`candidates` 按 `last_accessed_at` 排序，与「是否过期」无关，而 `remaining` 从未扣除 TTL 已选体积的 `total` 起算 ⇒ 排在前面的**未过期活跃缓存**会被误删，哪怕只删过期文件就已降到低水位。更要紧的是**默认配置（24h + 10240 MB）两段都开**，这是最常见的运行形态，而两条既有测试各把另一段关成 0，从未覆盖它。ZCode 未发现此条，按更严格的一侧处理。 | `cache_manager.rs` 的 TTL 段改为 `SELECT file_path, file_size` 并累计 `ttl_freed`；容量段 `remaining = total.saturating_sub(ttl_freed)`，若已 `<= low` 则整段跳过；遍历 `candidates` 时**跳过已在 victims 中的条目**（既不扣减也不 push，避免重复计入）。新增 `policy(24, 10)` 两段同开的集成单测：过期文件足以降到水位时，未过期文件必须保留。 |
| AGY-02 | gemini | 次要 | 接受 | 属实，且是我自己的执行遗漏：上一轮专门为 `history_max_entries` / `transfer_card_retain_secs` 写了 `legacy_settings_json_deserializes_without_data_loss` 这道闸，并验证过「换回裸 `#[serde(default)]` 会变红」；这轮加了三个同类字段却没扩展它。字段文档里写着「后果最严重」，却恰恰没有守护。 | 与 GLM-01 合并处置，见该行落点。 |
| AGY-03 | gemini | 吹毛求疵 | 接受 | 属实。`cache_manager.rs:108` 的 `prune_history` 文档注释写「与 `sweep_expired_and_lru` 同序」，而本轮已将该函数改名为 `sweep`，全仓仅剩这一处旧名引用，读者按名字找不到函数。 | 与 GLM-05 合并处置，见该行落点。 |
| GLM-01 | glm | 次要 | 接受 | 与 AGY-02 同一缺陷、同一依据（两侧独立命中）。GLM 补充了一条关键观察：`retention.rs` 的单测都经 `default_config()` 显式构造，**走不到 serde 缺字段路径**，所以那 14 条测试一条都测不到这个回归——这解释了为什么我会以为已经覆盖。 | `settings_cmd.rs`：`legacy_settings_json_deserializes_without_data_loss` 补三条断言（缺字段时得到 24 / 10240 / 60 而非 0）；`explicit_zero_is_preserved_not_defaulted` 补 `cache_ttl_hours: 0`、`cache_max_size_mb: 0` 的保留断言；`current_settings_json_round_trips` 补三字段往返。补完后按惯例做回退验证（换回裸 default 必须变红）。 |
| GLM-02 | glm | 次要 | 接受 | 属实，且 GLM 对残余风险的判断准确：文件在第 2 段已删，复核救不回文件，只能让行多留一轮自愈。真正的问题是**不对称**——`prune_history` 对同一窗口既做了第 3 段复核，又把局限明文写进注释；而 `sweep` 两者都没有，却在文档里引用 `history_pruner` 的锁纪律总纲。自己引用的纪律要么照做要么说明为何不做，不能只引用不兑现。 | `cache_manager.rs` 的 `sweep` 第 3 段：删行前用 `HistoryRepo::filter_recently_injected` 同款判据按 `file_path` 复核一次（与 `prune_history` 对齐），并在 `sweep` 的文档注中如实写明「复核只挡得住行被删，文件已在第 2 段删除、救不回来」，坦率程度与既有说明一致。 |
| GLM-03 | glm | 次要 | 接受 | 属实，而且构成对本次重构自身理由的反例：整段重写的动机就是「持锁停顿会把整条信令链路一起堵住」，却在持锁区间内放了一个对持续增长的 `Vec` 做线性扫描的循环，最坏 O(n²) 字符串比较。剪贴板类应用的典型形态正是海量小文件，且当免疫文件让配额无法满足时会遍历全部候选——恰好是最坏情况。 | 成员判断改用 `HashSet<String>`（进入循环前由 victims 构建一次，均摊 O(1)）；第 3 段的 DELETE 循环改用 `tx.prepare` 复用 prepared statement，同样在持锁区间内。 |
| GLM-04 | glm | 次要 | 接受 | 属实且我沿用了旧写法：`if let Ok(purged)` 把 `Err` 整个丢弃，不留任何日志。后台清理的唯一职责就是防磁盘占满，一旦持续失败（第 1 段 SQL 错误、第 3 段事务失败都会提前返回 Err），外部零信号——**故障呈现形态恰是这个功能本身要防的「磁盘被无声占满」**。同为后台维护入口的 `prune_and_notify` 对 Err 记 `log::warn`，两处不对称。 | `lib.rs` 的清理循环改为 `match`：`Ok(n) if n > 0` 记 info，`Err(e)` 记 `log::warn!`，与 `history_pruner.rs:45` 的处理对齐。 |
| GLM-05 | glm | 吹毛求疵 | 接受 | 与 AGY-03 同源（两侧独立命中）。GLM 额外用全目录 grep 确认了「仅剩这一处函数引用，`retention.rs:3-4` 的命中是对旧常量历史的叙述性提及而非函数引用」，并提醒顺带核对「同序」表述对三段式后的 `sweep` 是否仍成立——核对结论是仍成立（先删文件后删行）。 | `cache_manager.rs:108` 的旧名改为 `sweep`；保留「同序」表述（经核对仍然成立）。 |

**裁决统计**：接受 8 ｜ 驳回 0 ｜ 暂缓 0。

---

## 3. 修改方案（待审批后执行）

按契约，闸门期内不动代码。审批通过后按下列顺序执行。

### 3.1 `cache_manager.rs` —— 过度删除（AGY-01，优先级最高）

```rust
// TTL 段：连 size 一起取，累计已释放体积
let mut ttl_freed: u64 = 0;
if policy.ttl_enabled() {
    // SELECT file_path, file_size ...
    for (path, size) in expired { ttl_freed += size; victims.push(path); }
}

if policy.quota_enabled() {
    let total = SUM(file_size);
    // 先扣除 TTL 已选中的，再判断是否仍然超限
    let mut remaining = total.saturating_sub(ttl_freed);
    let low = policy.low_watermark_bytes();
    if remaining > low {
        let victim_set: HashSet<String> = victims.iter().cloned().collect();  // GLM-03
        for (path, size) in candidates {
            if remaining <= low { break; }
            if victim_set.contains(&path) { continue; }  // 已计入，不重复扣减
            remaining = remaining.saturating_sub(size);
            victims.push(path);
        }
    }
}
```

### 3.2 `cache_manager.rs` —— 第 3 段复核 + 性能（GLM-02 / GLM-03）

删行前按 `file_path` 复核剪贴板免疫状态，并复用 prepared statement；
文档注补写该窗口的局限。

### 3.3 `lib.rs` —— 错误不再静默（GLM-04）

```rust
match cache_sweep_mgr.sweep(policy).await {
    Ok(n) if n > 0 => log::info!("Cache cleaner purged {} entries", n),
    Ok(_) => {}
    Err(e) => log::warn!("Cache sweep failed: {}", e),
}
```

### 3.4 `settings_cmd.rs` —— 补齐 serde 守护（AGY-02 / GLM-01）

扩展三个既有测试，补完后做回退验证。

### 3.5 `cache_manager.rs:108` —— 陈旧注释（AGY-03 / GLM-05）

### 3.6 新增测试

- **两段同开**（AGY-01 的守护）：`policy(24, 10)`，构造 §1 的反例——
  过期文件足以降到低水位时，未过期的活跃文件必须保留；
- 第 3 段免疫复核（GLM-02）；
- serde 三字段的缺省 / 显式 0 / 往返（AGY-02 / GLM-01）。

### 3.7 验证

`cargo test`（预期 57 → 61 左右）、`pnpm test`、`tsc --noEmit`、`pnpm build`；
关键测试逐条做回退注入验证。

---

## 4. 仍未覆盖（如实记录）

- **端到端手工验收一次未跑**（修订计划 §4.4 的 4 步，含「保存后重启应用」前置）。
  两侧也都把这列为盲区。
- GLM-02 的窗口按 §2 只是**收窄**：文件仍会在第 2 段被删，复核只保住数据库行。
- `last_accessed_at` 无更新路径、所谓 LRU 实为「注册序」——既有缺陷，
  计划阶段已裁决本轮不修，两侧本轮均未再提。
