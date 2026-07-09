# 决策与理由

本文档记录 M0–M5 / P0–P7 每个里程碑的关键决策与**为什么这样不那样**。改一个已有设计前
先读对应条目——很多看起来"可以更简单"的方案已经被否决过,理由在这里。

> turn 号指向 Claude Code 会话 `aeae847e...` 的消息序号(见 [README.md](./README.md) 末尾)。

## 跨里程碑的根本决策

### 为什么是 Rust / Ratatui,不是 Go / Charm 或 TS / OpenTUI

决定性约束是**耳机门**:需要(1)按名枚举输出设备 (2)检测绑定耳机在场 (3)把播放流路由到
该具体设备(即使它不是 macOS 默认输出)。`cpal` 原生支持三者且零 CGO;Go 的 `beep`/`oto`
只能播到系统默认,唯一能选设备的 `malgo` 需 CGO 且低层回调,抵消 Go 简单性优势;TS 音频
故事最弱。(turn 43, 70)

### 为什么一期只做英语(不做政治/数学/408)

备考 2028 考研(南大 CS 专硕,初试 2027-12,~17 个月窗口,每日 ~15 词即够)。四门(101 政治 /
201 英语一 / 301 数学一 / 408)里,英语一是唯一"记忆密集 + 短板 + 适配静默 TUI"的科目,
ROI 最高;政治大纲每年 9 月才更新,7 月做政治词卡等于背要作废的;数学 / 408 是流体智力主场
不需要此 app。(turn 59, 70-72)

### 静态 / 动态内容划界(星火接线综合 thesis)

- **静态** = 词的客观知识(词素/词源/推导/例句/图边),对所有人都一样 → 一次烘焙、提交、内嵌。
- **动态** = 词与"你这颗脑子"的连接(锚点选择、推理点评、confusion memory)→ live。
- **个性化不是 LLM 填的字段,是数据库 JOIN 算出来的**;**诚实不是 LLM 贴的标,是 pipeline 从
  citation 算出来的**。(turn 974-985)

这条 thesis 直接驱动了 P0–P3 的所有重构。

### Mirror, not crutch(贯穿 LLM 触点)

DeepSeek 只交出词素 + 你可能已掌握的锚点 + derive-it-yourself 谜题,**绝不交一段让你被动吸收的
段落**。这个哲学同样支配产品对用户的说话方式。(turn 42, 366, 743)

### 设计评审团拒绝"85% rule"

称其为"misappropriated ML result dressed as human-learning science — the same sin as a hallucinated
root"。(turn 981)

## M0 — 耳机门 + CoreAudio 枚举(turn 126-200)

- **删直接 cpal 依赖,改用 `rodio::cpal` 重导出**:rodio 0.22 vendor 了 cpal 0.17.3,直接依赖
  cpal 0.18 会导致 `Device` 类型不匹配(E0308)。(turn 155-160)
- **CoreAudio 用显式 fourcc 常量**:版本无关,不猜 `coreaudio-sys` 重导出哪些符号。(turn 121)
- **耳机门是物理保证不是 if**:`RoutedPlayer` 把流开在绑定设备上,手里没有指向扬声器的流,
  漏音在物理上不可能。fail-closed 静音,绝不回退扬声器。(turn 130, 200)

## M1 — ECDICT → 考研牌组 + FSRS/SQLite(turn 256-296)

- **FSRS 定位为"镜子"**:建模记忆(difficulty/stability/retrievability)并报告何时到期,**不决定
  如何学**;把 grade 映射到 rating 交它调度。(turn 259)
- **dead_code 警告用 `#![allow(dead_code)]` 限定在 data 模块 + 注释**:M1 的 8 条死代码是 M2 review
  loop 的前向声明,不反复 churn。(turn 281)

## M2 — Ratatui 复习循环(turn 316-376)

- **同步 Ratatui 模型**:review loop 不需要 async runtime;后台工作(LLM、音频)通过 channel 到达;
  gate 每 ~1s 重轮询。(turn 321)
- **`render-preview` 用 TestBackend**:把真实屏幕渲成文本,无 TTY 下可验证 UI。(turn 376)

## M3 — DeepSeek 精加工 + 词根图 + 拆·联 UI(turn 391-523)

- **per-word enrichment 失败非致命 + 跳过非 deck 词**:`government` 触发 `max_tokens` 截断 JSON、
  `circumscribe` 不在 deck 触发 FK constraint 中断整批 → 抬升 token 上限、单词失败不终止、
  `has_word` 过滤。(turn 461-475)
- **enrichment token 上限抬到 ~4000+**:polysemous 词(state/government)输出长对象,留足空间不让
  JSON 截断 mid-object。(turn 468)
- **DeepSeek 模型名经 API 实查**:实际是 `deepseek-v4-flash` / `deepseek-v4-pro`,不信研究结论
  (V4 在训练截止后发布)。(turn 378-382)
- **reqwest 用 `default-tls` 而非 rustls-tls**:`rustls-tls` 不是有效 feature 名(导致 `cargo add` 静默
  失败),default-tls 在 macOS 最干净无需配 crypto-provider。(turn 439)

## M4 — Kokoro TTS + 耳机门播放(turn 534-633)

- **Kokoro 选 int8 ONNX + voices.bin**:92MB+28MB,可接受。(turn 526-543)
- **`on_key` 拆 Space/Enter**:Space 专管发音,Enter 专管揭示(原代码 `\n|\r|' '` 同一分支)。(turn 581)
- **懒加载 TTS 替代预合成**:冷启动 ~6s 是绕开原因,正确解法是常驻热进程模型只加载一次;
  `tuna synth` 降级为可选。(turn 810)

## M5 — 词根图谱浮现 + 苏格拉底辨析(turn 636-718)

- **`learned_siblings` 查已学同根词**:从 ECDICT `exchange` 字段实时解析改为基于 morpheme 的查询。
- **tui-markdown 选型**:0.3.8 依赖 `ratatui-core ^0.1`,与项目 ratatui 0.30 同 core → `Text`/`Line`/`Span`
  是同一类型,无版本坑;`--no-default-features` 砍掉 syntect 重依赖。(turn 771)
- **tui-big-text 弃用**:锁 ratatui 0.29(非 core),版本耦合陷阱;且 8×8 字体撑不下 `circumscribe`。(turn 898)

## P0 — morpheme 脊柱 + 杀锚点谎言(turn 1022-1066)

- **cognate_root 不存储、查询时从 morpheme 节点派生**:把"哪些词同根"从冗余存储改为查询时 JOIN,
  避免脏数据;原 JOIN 静默坏掉返回空正是 cognate_root 存储模型本身有问题。(turn 1022)
- **`known_anchors` 必须删**:个人化必须来自 live JOIN 学习者真实 FSRS 状态,绝不 baked;LLM 注入
  伪造的"该学习者已掌握"是谎言。(turn 1043-1045)
- **图重建而非迁移**:schema 演化(edge.via→why_zh、加 morpheme spine),迁移成本高于重建。(turn 1039)

## 可移植性重构 — ~/.tuna 单目录(turn 1077-1158)

- **全部归 `~/.tuna` 而非 XDG 分目录**:用户明确否决 XDG,要更简单;单目录跨设备一致,二进制旁
  不存任何东西。(turn 1081-1085)
- **不要单独 `tuna init` 命令**:启动即检测、空则自动初始化。(turn 1081)
- **4801 词典内嵌(非 851MB 全量 ECDICT)**:单文件自包含、`cargo install` 即用;范围内词典仅 2.2M。(turn 1076, 1103)

## P1a/b/c — Wiktionary 接地 + 关笼 LLM 烤制(turn 1163-1438)

- **P1 第一个交付物是镜子(覆盖率报告)而非直接烤**:在花 API 预算前先看清真实可分解率地形,
  据此定 S4 烤制范围;避免闷头烧钱。(turn 1072, 1163)
- **词源 RAG:喂 Wiktionary 模板给 LLM 当地基**:不让 LLM 凭记忆报词根(会幻觉);MediaWiki wikitext
  的 `{{affix}}`/`{{prefix}}`/`{{root}}`/`{{der}}` 显式编码真实词素;LLM 只负责"翻译真词源",flash 即够可靠。(turn 948-960)
- **two-stage 烘焙管线**:Stage 1 `bake.py`(确定性、零 LLM,产 `etym_cache.jsonl`)→ Stage 2 `narrate.py`
  (关笼 LLM,产 `morphemes.jsonl` + `enrichment.jsonl`);LLM 拿到已核验词素作为不可变真值,只能翻译+
  叙述+写例句,加不进也改不了任何根。(turn 1163, 1317)
- **single-root 词根锚定**:single-root 词(占 36%)以被引用的拉丁词源作为根节点锚入图(如
  `part → partem`),使其成为 partial/particle 的兄弟。(turn 1331-1337)
- **位置感知聚类 id**:保留连字符作为位置编码,`-al`(suffix)与 `al-`(prefix)不再假合并。(turn 1347-1357)
- **置信级由流水线从证据算出,非 LLM 自盖章**:每根可一键回溯 Wiktionary `rev_id`。(turn 1438)

### P1 踩的坑(避免重蹈)

- **94.8% no-page 误判**:Wiktionary 33 req/s 被 429 限流,fetcher 把 429 当"无页面"。修复:WORKERS=3
  + retries + backoff + `maxlag` 参数。(turn 1196-1219)
- **18.7% needs-review 解析器漏**:parser 只匹配裸 `{{af}}`,漏 Wiktionary 新 `{{ety|en|:af|...}}` 包装
  + 拉丁词源非首(parent 经古法语)+ inline `{{m|la|...}}`。修复后 needs-review 从 18.7% 降到 5.0%。(turn 1225-1249)
- **缓存 resume 不优先 raw ety**:加 raw 存储后,合并 tie-break 只比 category GOOD,旧"好但无 raw"条目
  被优先保留 → 判 0 命中、从头重抓。修复:`rank(x) = (category in GOOD, has raw ety)`。(turn 1298-1303)

## P2 — 星火接线挣得的边(turn 1442-1507)

- **节点只在被回忆时愈合,绝不在被显示时愈合**:机器永远不替你画新词与已知之间的边,它揭示
  `action = act + -ion`,然后问"哪个已学的词带 `act`";你在脑中回忆、翻牌、y/n 记一次那个老词的真
  FSRS refresh。"The edge you see is the one your mind just traversed."(turn 985, 1503)
- **Strike Prompt 阶段隐藏 siblings 列表**:防剧透回忆;只在 reveal 后弹 strike 提问。(turn 1473)
- **Strike 期间阻塞新词评分**:必须先解决回忆子交互,才能评新词。(turn 1457)

## P3 — guess-eval 苏格拉底活镜子(turn 1508-1532)

- **`a` 键点评用户推理而非泛型辨析**:把"用户猜的推导"变成活通道而非死回声;LLM 提引导性问题
  让用户自纠,再确认,绝不直接给判决。(turn 1514-1517)

## P4 — earned-strike arc 签名动画(turn 1538-1586)

- **不用 tachyonfx 做签名动画**:tachyonfx 0.19 钉 ratatui 0.29,项目用 0.30,版本冲突同 tui-big-text
  陷阱;用已有的 anim clock 自制轻量动画,零版本风险。(turn 1535-1538)
- **`w` 键打开 Wiktionary**:"honesty as a keypress"——每个根的引用证据一键可达。(turn 1538)

## 首启向导(turn 1589-1646)

- **首启向导 = 与"办公室静默"情感中心匹配的仪式感**,不是写空模板:三件事领着配好(绑耳机 /
  DeepSeek 密钥 / 发音模型)。(turn 1589/1593)
- **TTY 门用 `stdout.is_terminal()`(原 stdin)**:更可测,piped stdin 的 reads 仍能工作。(turn 1621)

## P5 — 星座 root-family overlay(turn 1654-1858)

- **只画真实存在的共享词素边,从不臆造**:四种 glow——`◉` teal=当前词,`✦` green=已学且 stability≥21d,
  `✦` amber=已学但尚新鲜,`·` muted=前沿暗星。(turn 1654-1858)
- **suffix 过滤用闭集 stoplist 而非 hyphen 启发式**:bake 把同一后缀有时写成 `-ion` 有时写成 `ion`,
  hyphen 启发漏过;语法后缀是有限集。(turn 1809)

## P6 — 同源合并(turn 1867-2076)

- **保守可审计的离线 pass,不做 fuzzy stemmer**:保护"诚实"红线(只画真实存在的共享词素边);
  不剥离拉丁屈折,只利用 `spect` 已是 `spectāculum` 的前缀这一事实 + gloss 重叠门
  (`port`→carry 不会吸收 `portion`→部分)。`spect` 家族从 2 词涨到 6 词。(turn 1867, 1947)
- **`bond` 提升为一等 schema 字段**:anchor/sibling/constellation 三处都要过滤语法后缀,复制过滤不
  集中;且合并后把 `ment` 规范化掉连字符会让 `best_anchor` 的 `kind_w`(原本靠 hyphen 区分 suffix
  0.35 / root 1.0)把后缀误升权到 root。集中字段杜绝该回归。(turn 2021)
- **strict-rank 父规则**:同源合并首版两个 bug——`spect` 与 `-spect` fold 相同留两 rival 根,且选了
  带连字符的 `-spect` 当 canonical;`praesidēns→prae` 是 root→prefix 错并。修复:等长 fold 允许,
  cleanest surface 胜出,prefix 列入黑名单不得作 root。(turn 1987-1996)

## P7 — 纯 Rust 单二进制(turn 2110-2370)

- **完全砍掉 Python/uv/espeak 而非保留可选 sidecar**:用户"选了最狠的路",整条 `文本→音素→token→
  ONNX→24kHz` 都进二进制,实现真正的单二进制自包含。(turn 2379, 9111-9112)
- **TTS 选 ort+misaki-rs(非 tract、非 Python)**:`ort 2.0-rc` 静态链 onnxruntime(macos 仅依赖系统框架);
  `misaki-rs default-features=false` 纯 Rust G2P、词典+POS 权重全编进二进制(~9MB),输出 Kokoro 训练
  用的 Misaki 音素集;`tract` 后端调研 agent 挂死,非必需。(turn 2110, 2168)
- **用 ort tuple form `(shape, &[T])` 而非 ndarray 视图**:ort rc.10 依赖 ndarray 0.16,项目原本 0.17,
  view 类型不匹配;tuple form 让 tuna 与 ort 的 ndarray 版本无版本耦合;完全移除 `ndarray` crate 依赖。(turn 2278-2294)
- **初始化改为阻塞式模型下载 + 进度条**(而非异步等待):用户明确要"前置条件都做完之后我们再进入
  主系统"。(turn 1870, 2379)

## 生产 release profile(turn 2396-2468)

- **release profile 必须显式调优**:`cargo build --release` 默认 `lto=false`/`codegen-units=16`/`strip="none"`
  留太多性能体积在桌上;生产二进制必须配 `[profile.release]`。(turn 2396-2399)
- **`panic = "unwind"` 而非 `abort`**:TUI 靠 unwinding 恢复终端(Drop guard 还原终端),`abort` 会跳过
  Drop 让终端烂掉。(turn 2416, 2468)
- **`target-cpu=native` 只用于个人安装**:可分发 build 不可用(不具可移植性),`.cargo/config.toml` 顶部
  注释钉死此约束。(turn 2416)
- **LightGBM / 个人 FSRS 权重拟合刻意押后**:都要真实复习数据才能训,新装零数据;`review_log` 表已
  就位等着喂。定性为"数据依赖,不是漏做"。(turn 2379)

## Windows 移植(未决,turn 2493-2503)

被阻塞两层:(1) ort 的 Windows 预编译库只有 `x86_64-pc-windows-msvc`,`cargo-zigbuild` 只能产
`windows-gnu`,zig 不能链 MSVC ABI 库 → 只要用 ort,zig 路死;(2) 更致命——tuna 现在是 macOS 专属
架构,`coreaudio-sys` + `core-foundation` 是无条件依赖,Windows 上 CoreAudio 不存在。Windows 版
"不是换个 linker 交叉编译,是一次移植"。需用户明示是否做,以及是否接受门语义降级。
