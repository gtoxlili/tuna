# 约定与红线

本文档列出不随情境变的**不变式**。前两节(产品红线、不可逆动作)是硬约束,任何改动都要
守住;后两节是 house style 与构建约定,保持一致性。

## 产品红线(硬不变式)

### 1. 耳机门:绝不回退扬声器

- 音频流**物理开在绑定的蓝牙耳机**上(`RoutedPlayer` 持有一条开在该 cpal 设备上的流),
  手里从来没有指向系统默认输出的流——漏音在物理上不可能,不是靠 `if` 拦。
- 绑定耳机不在场 ⇒ `find_output_device` 返回 `None` ⇒ **fail-closed 静音**,绝不回退扬声器。
- 绑定按**设备 UID**(跨重连稳定、内嵌 MAC,带 `:input`/`:output` 后缀),不按显示名——
  AirPods 同名输入/输出设备会出现两次,按名绑定会掷硬币。`probe` 会显式列出蓝牙输出设备及其 UID。
- **不可偷偷降级**:任何把"门控音频"改成"默认设备播放"的改动都等于漏音,背叛办公室静默初衷。
  若要做平台移植,必须用户明示同意,且重新定义该平台的门语义。

### 2. 词源诚实:LLM 无法编造词根

- 词素(morpheme)来自 `scripts/bake.py` 对 **Wiktionary 模板的确定性解析**,LLM 拿到的是
  已核验真词素作为不可变真值——只能翻译、叙述、写例句,**加不进也改不了任何根**。
- `etymology_confidence`(cited / folk / mnemonic)由 `bake.py`/`narrate.py` **从证据算出**,
  不是 LLM 自盖章。每根可回溯 Wiktionary `rev_id`。
- 日耳曼不可分解词(`might`/`area`)老实标"不可分解、给钩子",不硬编假词根。
- `w` 键打开该词 Wiktionary 词源页——引用证据一键可达("honesty as a keypress")。

### 3. 个性化是 live JOIN,不是 baked 字段

- `known_anchors` 字段已删——任何"该学习者已掌握的同根/基础词" baked 进资产都是谎言
  (它来自伪造的 prompt,不是真实学习状态)。
- 个性化必须来自运行时 `learned_siblings` / `anchor_candidates` 查询真实 FSRS `introduced` 集合。
- 推导谜题(`DerivePuzzle`)的锚点选择在运行时算,不在精加工时定。

### 4. cognate 关系永不存储

- `edge` 表只存 word↔word 成对边(synonym / antonym / confusable)。**cognate_root 永不存**。
- 同根关系 = 两个 word 在 `word_morpheme` 共享 `morpheme_id` 的 JOIN,查询时派生。
- `morpheme.bond = 0` 的语法后缀(-ment/-tion…)**不构成推导之桥**,所有 anchor/sibling/constellation
  查询都带 `m.bond = 1` 过滤。

### 5. 静态 / 动态内容划界

- **静态**(词的客观知识:词素/推导/例句/图边)对所有人都一样 → 一次烘焙、提交进 `assets/`、内嵌进二进制。
  需可审、可改、零延迟、可离线。
- **动态**(用户猜测点评 / 辨析)才走 live DeepSeek。
- TTS 是派生物(发音无需审)→ 懒加载合成 + 落 WAV 缓存。

## 不可逆动作

- 不可逆操作前必须用户确认。
- 绝不外泄密钥(`DEEPSEEK_API_KEY` / config 里的 api_key)。
- 越权操作直接拒。

## House style

### 仓库与提交

- 开发在 `build/mvp` 分支;`main` 仅 LICENSE。
- commit message 格式:`feat(scope): 描述 (MN)` / `fix(scope): ...` / `chore: ...` / `docs: ...` / `refactor: ...`。
  里程碑 tag 用 `(M0)`..`(M5)` / `(P0)`..`(P7)`。
- `.gitignore`:`/target`、`/data/`、`/cache/`、`tuna.toml`、`config.local.toml`、`.env`、`__pycache__/`、`*.pyc`。
  `data/` 全是 `build-deck` 可再生产物,`cache/` 同理。提交前 `git status --short` 核验,别误提交大文件
  (216MB ECDICT zip 曾因 `.gitignore` 只排 `data/*.db` 被误 staged)。

### 命令可见性

- 用户面命令:`Study` / `Ask` / `DeckInfo` / `Probe` / `GateTest`(都 `ensure_ready()`)。
- 维护者 / dev 命令一律 `#[command(hide = true)]`:`BuildDeck` / `ExportDeck` / `Enrich` / `RenderPreview` / `Synth`。
- `Synth` 只产 WAV 不播放,作为 dev 工具避免测试时触发耳机门依赖。
- `Probe` / `GateTest` 不调 `ensure_ready()`,无需初始化即可跑。

### 依赖卫生(版本陷阱高发区)

- **用 crate 前先读其真实源码**,不猜 API。Rust crate 版本可能比文档假设的新。
- **ratatui 周边 crate**:采用前必查它依赖 `ratatui ^X.Y` 还是 `ratatui-core ^X.Y`。后者才与
  ratatui 0.30 兼容;前者钉旧版会版本冲突。`tui-big-text`(钉 0.29)、`tachyonfx`(钉 0.29)都因此弃用,
  改用自制动画。
- `tui-markdown` 用 `--no-default-features` 砍掉 syntect 重依赖,且它依赖 `ratatui-core` 与项目同 core。
- `ort = "=2.0.0-rc.10"` 精确 pin(RC 版本);用 ort 的 **tuple form `(shape, &[T])`** 而非 ndarray 视图,
  让 tuna 与 ort 内部的 ndarray 版本无耦合(ort rc.10 依赖 ndarray 0.16,直接用 view 会类型不匹配)。
- `cpal` 通过 `rodio::cpal` 重导出使用,不直接依赖——rodio 0.22 vendor 了 cpal 0.17.3,
  直接依赖 cpal 0.18 会导致 `Device` 类型不匹配(E0308)。
- `reqwest` 用 `default-tls`(macOS Secure Transport),`rustls-tls` 不是有效 feature 名会让 `cargo add` 静默失败。

### 代码约定

- schema 演化时**重建而非迁移**:`build_from_ecdict` 重建图,不写迁移脚本。
- `morphemes.jsonl` **不被运行时加载**(只 `enrichment.jsonl` 进库;morpheme id 来自 `normalize_morpheme`)。
- 保留连字符作为位置编码:`-al`(suffix)与 `al-`(prefix)是不同节点,normalize 不剥连字符。
- 视觉:deep-water instrument 调色板——深海墨底、phosphor-teal(`#34D3C2`)为"推导电流"主色、amber 为"你已拥有的部件"。

### 时间与 ETA

- 时间估计必须有**窗口测量**支撑,不凭记忆估"几十分钟"。等待后台任务用 bounded `until`-loop
  (如 `until grep -qE 'Finished|error' "$F" || [ $i -ge 90 ]; do ...`),不用 `sleep N; tail` 链式。
- 外部 API 模型名 / 能力**实查**(`GET /models`),不信研究结论或知识截止。

## 构建约定

### release profile

[Cargo.toml](../Cargo.toml) `[profile.release]`:

```toml
opt-level = 3        # 最大运行时优化
lto = "fat"          # 全程序 LTO(含 ort/ratatui/rusqlite)
codegen-units = 1    # 单单元,优化器看到全部
strip = "symbols"    # 去 symbol table + debuginfo
panic = "unwind"     # 刻意保留:TUI 靠 unwinding 恢复终端;abort 会跳过 Drop 弄烂终端
```

`cargo build --release` 默认 `lto=false`/`codegen-units=16`/`strip="none"` 留太多性能体积在桌上,
生产二进制必须显式配此 profile。

### target flags([.cargo/config.toml](../.cargo/config.toml))

- `aarch64-apple-darwin`:`target-cpu=native`(本机构建本机跑,个人 `~/.local/bin` 安装最优)。
  **不可用于可分发的 macOS build**,不具可移植性——此约束在 config.toml 顶部注释钉死。
- Linux gnu/musl:`mold` linker + `x86-64-v3` 基线(AVX2-era,~2015+),需 build host 装 mold。
- 这些 Linux 段在 macOS 上 inert,只在交叉编译时激活。

### 验证模式

- `cargo build` 验证:`grep -E '^error' -A5 | head -50` + `grep -E '^warning:|^error|Finished' | head`。
- release 构建后烟测 `tuna synth`(LTO 后音频管线仍健康;首次合成从 debug ~1.6s 降到 release ~0.6s)。
- UI 改动用 `tuna render-preview [--word <w>]`(TestBackend 把真实屏幕渲成文本,无 TTY 可验证)。
