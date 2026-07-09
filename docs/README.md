# tuna · 开发文档

这份文档面向**后续接手的开发 AI**(以及人类维护者)。tuna 是一个 Rust 写的终端应用,
让用户在办公室里通过绑定的蓝牙耳机、以"推导"而非"死记"的方式学考研英语词汇。产品
的情感中心是**办公室静默**——音频只从绑定的耳机出,物理上不可能漏音。

> 仓库根的 `README.md` 面向**用户**(快速上手);这里的 `docs/` 面向**改代码的人**。
> 两者有少量重叠,但 docs/ 聚焦"为什么这样设计、动代码时要注意什么"。

## 文档清单

| 文件 | 何时读 |
|---|---|
| [architecture.md](./architecture.md) | 动任何代码之前。模块图、数据模型、运行时模型、关键不变式。 |
| [decisions.md](./decisions.md) | 想改一个已有设计、或好奇"为什么这样不那样"时。M0–M5 / P0–P7 每个里程碑的关键决策与理由。 |
| [conventions.md](./conventions.md) | 写新代码或提交之前。红线(安全/不可逆)、house style、构建约定。 |
| [data-pipeline.md](./data-pipeline.md) | 碰 `assets/*.jsonl` 或 `scripts/{bake,narrate}.py` 时。离线资产烘焙管线。 |
| [backlog.md](./backlog.md) | 接新需求之前。已完成、已知缺口、待决问题。 |

## 30 秒上手

- **是什么**:终端里的考研英语词根推导学习工具。方法论「拆·联·验」(Decompose · Link · Retrieve)+ FSRS 间隔重复。
- **技术栈**:纯 Rust 单二进制 · Ratatui(TUI)· cpal/rodio(音频)· rusqlite · rs-fsrs · ort + misaki-rs(Kokoro 本地 TTS)· DeepSeek(辨析)· ECDICT(离线词典,内嵌)。
- **运行**:`cargo run --` 或 `cargo install --path . && tuna`。首次运行三步向导(绑耳机 / 密钥 / 下模型),之后学习离线可用。
- **数据根**:`~/.tuna/`(`config.toml` / `tuna.db` / `cache/audio/` / `tts/`),`$TUNA_HOME` 可覆盖(测试用)。
- **分支**:`build/mvp`(开发主线),`main` 仅有 LICENSE。

## 关键不变式(改代码前必读)

这几条是**不随情境变的产品红线**,详见 [conventions.md](./conventions.md):

1. **耳机门**:音频流物理开在绑定的蓝牙耳机上,手里从来没有指向扬声器的流;耳机不在场 ⇒ 零音频(fail-closed 静音),绝不回退扬声器。
2. **词源诚实**:词素来自 Wiktionary 确定性管线,LLM 只翻译 / 叙述已核验的真词素,结构上无法编造词根。
3. **个性化是 live JOIN,不是 baked 字段**:`known_anchors` 已删;任何"该学习者已掌握"必须来自运行时 `learned_siblings` 查询真实 FSRS 状态。
4. **cognate(同根)关系永不存储**:查询时从共享 morpheme 节点 JOIN 派生。

## 这份文档的来源

由 orchestrator 编排 5 个子 agent 梳理 Claude Code 会话
`aeae847e-c762-4426-98ef-f6f9f5db465b`(678 user / 1463 assistant 轮,2513 条 jsonl)后综合而成。
原始 jsonl 在 `~/.claude/projects/-Users-gt-Downloads-tuna/aeae847e-c762-4426-98ef-f6f9f5db465b.jsonl`。
需要回溯某条决策时,[decisions.md](./decisions.md) 里附的 `turn N` 指向该 jsonl 的对应消息序号。
