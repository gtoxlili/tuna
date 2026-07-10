# 决策与理由

本文档记录 M0–M5 / P0–P7 每个里程碑的关键决策与**为什么这样不那样**。改一个已有设计前
先读对应条目，很多看起来"可以更简单"的方案已经被否决过,理由在这里。

## 跨里程碑的根本决策

### 为什么是 Rust / Ratatui,不是 Go / Charm 或 TS / OpenTUI

决定性约束是**耳机门**:需要(1)按名枚举输出设备 (2)检测绑定耳机在场 (3)把播放流路由到
该具体设备(即使它不是 macOS 默认输出)。`cpal` 原生支持三者且零 CGO;Go 的 `beep`/`oto`
只能播到系统默认,唯一能选设备的 `malgo` 需 CGO 且低层回调,抵消 Go 简单性优势;TS 音频
故事最弱。

### 为什么一期只做英语(不做政治/数学/408)

备考 2028 考研(南大 CS 专硕,初试 2027-12,~17 个月窗口,每日 ~15 词即够)。四门(101 政治 /
201 英语一 / 301 数学一 / 408)里,英语一是唯一"记忆密集 + 短板 + 适配静默 TUI"的科目,
ROI 最高;政治大纲每年 9 月才更新,7 月做政治词卡等于背要作废的;数学 / 408 是流体智力主场
不需要此 app。

### 静态 / 动态内容划界(星火接线综合 thesis)

- **静态** = 词的客观知识(词素/词源/推导/例句/图边),对所有人都一样 → 一次烘焙、提交、内嵌。
- **动态** = 词与"你这颗脑子"的连接(锚点选择、推理点评、confusion memory)→ live。
- **个性化不是 LLM 填的字段,是数据库 JOIN 算出来的**;**诚实不是 LLM 贴的标,是 pipeline 从
  citation 算出来的**。

这条 thesis 直接驱动了 P0–P3 的所有重构。

### LLM 只做镜子不做拐杖

DeepSeek 只交出词素 + 你可能已掌握的锚点 + derive-it-yourself 谜题,**不交让你被动吸收的段落**。

### 拒绝"85% rule"

85% rule 是对 ML 研究结果的误用,套到人类学习上不成立,和臆造词根是同一类错误。

## M0 — 耳机门 + CoreAudio 枚举

- **删直接 cpal 依赖,改用 `rodio::cpal` 重导出**:rodio 0.22 vendor 了 cpal 0.17.3,直接依赖
  cpal 0.18 会导致 `Device` 类型不匹配(E0308)。
- **CoreAudio 用显式 fourcc 常量**:版本无关,不猜 `coreaudio-sys` 重导出哪些符号。
- **耳机门是物理保证不是 if**:`RoutedPlayer` 把流开在绑定设备上,不指向扬声器的流,
  漏音在物理上不可能。fail-closed 静音,不回退扬声器。

## M1 — ECDICT → 考研牌组 + FSRS/SQLite

- **FSRS 定位为"镜子"**:建模记忆(difficulty/stability/retrievability)并报告何时到期,**不决定
  如何学**;把 grade 映射到 rating 交它调度。
- **dead_code 警告用 `#![allow(dead_code)]` 限定在 data 模块 + 注释**:M1 的 8 条死代码是 M2 review
  loop 的前向声明,不反复 churn。

## M2 — Ratatui 复习循环

- **同步 Ratatui 模型**:review loop 不需要 async runtime;后台工作(LLM、音频)通过 channel 到达;
  gate 每 ~1s 重轮询。
- **`render-preview` 用 TestBackend**:把真实屏幕渲成文本,无 TTY 下可验证 UI。

## M3 — DeepSeek 精加工 + 词根图 + 拆·联 UI

- **per-word enrichment 失败非致命 + 跳过非 deck 词**:`government` 触发 `max_tokens` 截断 JSON、
  `circumscribe` 不在 deck 触发 FK constraint 中断整批 → 抬升 token 上限、单词失败不终止、
  `has_word` 过滤。
- **enrichment token 上限抬到 ~4000+**:polysemous 词(state/government)输出长对象,留足空间不让
  JSON 截断 mid-object。
- **DeepSeek 模型名经 API 实查**:实际是 `deepseek-v4-flash` / `deepseek-v4-pro`,不信研究结论
  (V4 在训练截止后发布)。
- **reqwest 用 `default-tls` 而非 rustls-tls**:`rustls-tls` 不是有效 feature 名(导致 `cargo add` 静默
  失败),default-tls 在 macOS 最干净无需配 crypto-provider。

## M4 — Kokoro TTS + 耳机门播放

- **Kokoro 选 int8 ONNX + voices.bin**:92MB+28MB,可接受。
- **`on_key` 拆 Space/Enter**:Space 专管发音,Enter 专管揭示(原代码 `\n|\r|' '` 同一分支)。
- **懒加载 TTS 替代预合成**:冷启动 ~6s 是绕开原因,正确解法是常驻热进程模型只加载一次;
  `tuna synth` 降级为可选。

## M5 — 词根图谱浮现 + 苏格拉底辨析

- **`learned_siblings` 查已学同根词**:从 ECDICT `exchange` 字段实时解析改为基于 morpheme 的查询。
- **tui-markdown 选型**:0.3.8 依赖 `ratatui-core ^0.1`,与项目 ratatui 0.30 同 core → `Text`/`Line`/`Span`
  是同一类型,无版本坑;`--no-default-features` 砍掉 syntect 重依赖。
- **tui-big-text 弃用**:锁 ratatui 0.29(非 core),版本耦合陷阱;且 8×8 字体撑不下 `circumscribe`。

## P0 — morpheme 脊柱 + 杀锚点谎言

- **cognate_root 不存储、查询时从 morpheme 节点派生**:把"哪些词同根"从冗余存储改为查询时 JOIN,
  避免脏数据;原 JOIN 静默坏掉返回空正是 cognate_root 存储模型本身有问题。
- **`known_anchors` 必须删**:个人化必须来自 live JOIN 学习者真实 FSRS 状态,绝不 baked;LLM 注入
  伪造的"该学习者已掌握"是谎言。
- **图重建而非迁移**:schema 演化(edge.via→why_zh、加 morpheme spine),迁移成本高于重建。

## 可移植性重构 — ~/.tuna 单目录

- **全部归 `~/.tuna` 而非 XDG 分目录**:用户明确否决 XDG,要更简单;单目录跨设备一致,二进制旁
  不存任何东西。
- **不要单独 `tuna init` 命令**:启动即检测、空则自动初始化。
- **4801 词典内嵌(非 851MB 全量 ECDICT)**:单文件自包含、`cargo install` 即用;范围内词典仅 2.2M。

## P1a/b/c — Wiktionary 接地 + 关笼 LLM 烤制

- **P1 第一个交付物是镜子(覆盖率报告)而非直接烤**:在花 API 预算前先看清真实可分解率地形,
  据此定 S4 烤制范围;避免闷头烧钱。
- **词源 RAG:喂 Wiktionary 模板给 LLM 当地基**:不让 LLM 凭记忆报词根(会幻觉);MediaWiki wikitext
  的 `{{affix}}`/`{{prefix}}`/`{{root}}`/`{{der}}` 显式编码真实词素;LLM 只负责"翻译真词源",flash 即够可靠。
- **two-stage 烘焙管线**:Stage 1 `bake.py`(确定性、零 LLM,产 `etym_cache.jsonl`)→ Stage 2 `narrate.py`
  (关笼 LLM,产 `morphemes.jsonl` + `enrichment.jsonl`);LLM 拿到已核验词素作为不可变真值,只能翻译+
  叙述+写例句,加不进也改不了任何根。
- **single-root 词根锚定**:single-root 词(占 36%)以被引用的拉丁词源作为根节点锚入图(如
  `part → partem`),使其成为 partial/particle 的兄弟。
- **位置感知聚类 id**:保留连字符作为位置编码,`-al`(suffix)与 `al-`(prefix)不再假合并。
- **置信级由流水线从证据算出,非 LLM 自盖章**:每根可一键回溯 Wiktionary `rev_id`。

### P1 踩的坑(避免重蹈)

- **94.8% no-page 误判**:Wiktionary 33 req/s 被 429 限流,fetcher 把 429 当"无页面"。修复:WORKERS=3
  + retries + backoff + `maxlag` 参数。
- **18.7% needs-review 解析器漏**:parser 只匹配裸 `{{af}}`,漏 Wiktionary 新 `{{ety|en|:af|...}}` 包装
  + 拉丁词源非首(parent 经古法语)+ inline `{{m|la|...}}`。修复后 needs-review 从 18.7% 降到 5.0%。
- **缓存 resume 不优先 raw ety**:加 raw 存储后,合并 tie-break 只比 category GOOD,旧"好但无 raw"条目
  被优先保留 → 判 0 命中、从头重抓。修复:`rank(x) = (category in GOOD, has raw ety)`。

## P2 — 星火接线

- **节点只在被回忆时愈合,绝不在被显示时愈合**:机器永远不替你画新词与已知之间的边,它揭示
  `action = act + -ion`,然后问"哪个已学的词带 `act`";你在脑中回忆、翻牌、y/n 记一次那个老词的真
  FSRS refresh。
- **Strike Prompt 阶段隐藏 siblings 列表**:防剧透回忆;只在 reveal 后弹 strike 提问。
- **Strike 期间阻塞新词评分**:必须先解决回忆子交互,才能评新词。

## P3 — guess-eval 苏格拉底活镜子

- **`a` 键点评用户推理而非泛型辨析**:把用户猜的推导发给 DeepSeek 做苏格拉底式点评，LLM 提引导性问题
  让用户自纠，不直接给判决。

## P4 — strike 动画

- **不用 tachyonfx 做签名动画**:tachyonfx 0.19 钉 ratatui 0.29,项目用 0.30,版本冲突同 tui-big-text
  陷阱;用已有的 anim clock 自制轻量动画,零版本风险。
- **`w` 键打开 Wiktionary**:每个根的引用证据一键可达。

## 首启向导

- **首启向导是三步交互**(绑耳机 / DeepSeek 密钥 / 发音模型)，不是写空模板。
- **TTY 门用 `stdout.is_terminal()`(原 stdin)**:更可测,piped stdin 的 reads 仍能工作。

## P5 — 星座 root-family overlay

- **只画真实存在的共享词素边,从不臆造**:四种 glow：`◉` teal=当前词,`✦` green=已学且 stability≥21d,
  `✦` amber=已学但尚新鲜,`·` muted=前沿暗星。
- **suffix 过滤用闭集 stoplist 而非 hyphen 启发式**:bake 把同一后缀有时写成 `-ion` 有时写成 `ion`,
  hyphen 启发漏过;语法后缀是有限集。

## P6 — 同源合并

- **保守可审计的离线 pass,不做 fuzzy stemmer**:保护"诚实"红线(只画真实存在的共享词素边);
  不剥离拉丁屈折,只利用 `spect` 已是 `spectāculum` 的前缀这一事实 + gloss 重叠门
  (`port`→carry 不会吸收 `portion`→部分)。`spect` 家族从 2 词涨到 6 词。
- **`bond` 提升为一等 schema 字段**:anchor/sibling/constellation 三处都要过滤语法后缀,复制过滤不
  集中;且合并后把 `ment` 规范化掉连字符会让 `best_anchor` 的 `kind_w`(原本靠 hyphen 区分 suffix
  0.35 / root 1.0)把后缀误升权到 root。集中字段杜绝该回归。
- **strict-rank 父规则**:同源合并首版两个 bug：`spect` 与 `-spect` fold 相同留两 rival 根,且选了
  带连字符的 `-spect` 当 canonical;`praesidēns→prae` 是 root→prefix 错并。修复:等长 fold 允许,
  cleanest surface 胜出,prefix 列入黑名单不得作 root。

## P7 — 纯 Rust 单二进制

- **完全砍掉 Python/uv/espeak 而非保留可选 sidecar**:用户选了纯 Rust 路径，整条 `文本→音素→token→
  ONNX→24kHz` 都进二进制,实现真正的单二进制自包含。
- **TTS 选 sherpa-onnx(非 ort+misaki-rs 自组合)**:`sherpa-onnx` crate 静态链接 k2-fsa 维护的 C++ 库,
  一个 `OfflineTts` API 覆盖 Kokoro(风格向量)/ Matcha(条件流匹配)/ Piper(VITS 社区多音色)三引擎,
  G2P 走内嵌 espeak-ng-data。相比 ort+misaki 自组合:省掉自己写 ONNX 推理 session 管理 + G2P 预处理,
  引擎切换零代码改动(只换 model config 路径),且 sherpa 预编译包含 espeak-ng-data 用户无需系统 espeak。
  build script 从 GitHub releases 拉预编译库;本地 TLS 故障可用 `SHERPA_ONNX_ARCHIVE_DIR` 指向缓存绕过。
 
- **引擎描述符拆三个文件而非巨型 match**:`kokoro.rs`/`matcha.rs`/`piper.rs` 各自持有自己的 URL/voice/
  footprint/files() 布局,`TtsEngine` trait 统一静态描述符接口，加新引擎只加一个文件 + enum 变体,
  不动 session 合成逻辑。
- **`gen` 是 Rust 2024 reserved keyword**:session 里 `GenerationConfig` 变量不能用 `gen`,改用 `gen_cfg`。
- **初始化改为阻塞式模型下载 + 进度条**(而非异步等待):用户明确要"前置条件都做完之后我们再进入
  主系统"。

## 生产 release profile

- **release profile 必须显式调优**:`cargo build --release` 默认 `lto=false`/`codegen-units=16`/`strip="none"`
  留太多性能体积在桌上;生产二进制必须配 `[profile.release]`。
- **`panic = "unwind"` 而非 `abort`**:TUI 靠 unwinding 恢复终端(Drop guard 还原终端),`abort` 会跳过
  Drop 让终端烂掉。
- **`target-cpu=native` 只用于个人安装**:可分发 build 不可用(不具可移植性),`.cargo/config.toml` 顶部
  注释钉死此约束。
- **LightGBM / 个人 FSRS 权重拟合刻意押后**:都要真实复习数据才能训,新装零数据;`review_log` 表已
  就位等着喂。定性为"数据依赖,不是漏做"。

## 跨平台移植(已落地)

原 Windows 移植被阻塞两层:(1) ort 的 Windows 预编译库只有 `x86_64-pc-windows-msvc`,
`cargo-zigbuild` 只能产 `windows-gnu`,zig 不能链 MSVC ABI 库 → 只要用 ort,zig 路死;(2) tuna 是
macOS 专属架构,`coreaudio-sys` + `core-foundation` 是无条件依赖,Windows 上 CoreAudio 不存在。

解法是迁到 sherpa-onnx + cfg 门:

- **TTS 换 sherpa-onnx**:sherpa 静态链接 C++ 库,三平台(macOS/Linux/Windows)都有预编译包,ort 的
  MSVC-only / zig 不兼容问题消失。
- **CoreAudio 依赖收进 `cfg(target_os="macos")` 门**:`Cargo.toml` 把 `coreaudio-sys` + `core-foundation`
  移到 `[target.'cfg(target_os = "macos")'.dependencies]`,Linux/Windows 构建不拉这俩。
- **`trait AudioProbe` 抽象设备枚举**:`probe.rs` 按目标挑后端：macOS 走 CoreAudio HAL(UID + transport
  fourcc),Linux/Windows 走 cpal ALSA/WASAPI。无匹配目标 `compile_error!` 防漏。
- **门语义降级而非放弃**:cpal 0.17 在 Linux/Windows 不暴露稳定 UID / transport fourcc,所以非 macOS
  平台回退按显示名绑定(老实标 "可能随重启漂移,需重跑 setup 重绑"),但 **fail-closed 原则三平台一致**，
  绑定设备不在场照样零音频。这是用户明示同意的降级,不是偷偷回退扬声器。

三平台 cfg 门 + cpal 后端 + 按名字绑定降级落地后,跨平台移植完成。

## 交互系统重构 v3

对 v2 交互做系统级重评估后,确定以下决策:

- **Tab 命令菜单而非 which-key popup / `:` command palette**:用户明示"通过方向键等去进行交互"。
  which-key popup 仍字母驱动(Space+letter);`:` palette 6 个命令不需要 fuzzy 搜索。Tab 菜单是方向键
  驱动：Tab 在终端稳定(不像 F1/Ctrl 在 tmux 下被吞),↑↓ 选、Enter 确认、字母直达(lazygit 模式,专家
  零摩擦)、Esc/Tab 关。主路径不要求记忆 `a/g/s/w` 字母。
- **overlay 用扁平 bool + 纪律修复,不重构为 `Vec<Overlay>` 栈**:用户已决策"Esc=退一层"语义,扁平 bool
  的拦截顺序(help → settings → ask → graph → cmdmenu → strike → base)已是栈。bug 在纪律(静默吞键 /
  help 任意键关),不在架构。
- **方向键语义统一**:↑↓ = 纵向焦点移动(speak_cursor / 菜单选项 / 星座扁平 / 辨析滚动 / 引擎选择),
  ←→ = 横向(星座组内)或静默 no-op(无列表状态)。4 键不做同件事：revealed 阶段 ←→ 曾镜像 ↑↓,是
  语义噪声,改为静默 no-op 保留给未来横向用途。
- **Esc 语义分层**:done 状态单按即退(无未保存状态,两按过度);base 两按确认(2s 窗口,防误退丢复习
  状态);overlay 顶层退一层。**不**做 overlay 间 Tab 轮转，Esc 只退不进。
- **overlay 静默吞键改 toast**:ask/graph overlay 内按不可用键(1-4/a/w/s/hjkl)不再 `_ => {}` 静默吞,
  toast "先 a/Esc 关闭辨析" 给明确反馈。silent swallow 是 bug，用户不知是否生效。
- **help dismiss 纪律**:help 开时只 Esc/? 关闭,其他键**穿透到 underlying overlay**(不 return,继续走
  on_key 剩余逻辑)。原 help 任意键关闭,用户想对 underlying overlay 操作被迫按两次。
- **LLM generation 计数 + 120s 超时**:`ask_gen: u64` 计数,每次 `ask_socratic` 自增;worker 闭包捕获
  gen_id 通过 channel 发 `(gen_id, result)`;`poll_async` 校验 `gen_id == self.ask_gen` 才用结果。cancel
  后旧线程仍跑 + 计费的问题消失，stale 结果被丢弃。`reqwest` client 加 120s timeout。
- **grade_flash 是跨卡瞬态,不在 `load_current` 清**:grade() 设 grade_flash → pos+=1 → load_current 加载
  下一张;wash 携带到新卡前 ~250ms 作为"你按了哪个评分键"的反馈。A4 曾把它当 per-card 瞬态清掉,导致
  flash 永不显示，改为只清 strike_anim/ask/graph 等 per-card 状态,grade_flash 自过期(poll_async D6
  清理)+ undo_grade 显式清。
- **动画预算 ≤4 类,全 ≤400ms,全受 `reduced_motion` 门控**:卡片淡入 150ms / morpheme 错峰 60ms×index
  (每 cell 120ms fade)/ grade flash 250ms(从 350ms 缩)/ strike arc 400ms(从 900ms 缩，900ms 期间
  非 reduced 用户看 arc、reduced 用户看 siblings,两群体首 900ms 内容不同)。spinner 也尊重
  reduced_motion(reduced 时静态 "○")。
- **strike_anim 缩短 + siblings 总是渲染**:原 `else if` 在 arc 期间 siblings 消失(900ms 内容空洞);
  改为 siblings 总是渲染、arc 叠加在上。reduced 与非 reduced 用户读相同内容。
- **撤销评分 'u'(3s 窗口,Anki AJT 范式)**:意外按错 1/2/3/4 立刻切卡无法撤销。`undo_snap` 存 pre-grade
  快照(DeckCard + Instant),3s 内 u 键恢复 FSRS 状态 + pos 回退 + reload。超时 toast。单步不可链式
  (多步 undo 会让显示流与 FSRS review 历史分叉,破坏参数可信度)。
- **interval 中文单位 + 逾期修复**:human_interval 输出"分/时/天"而非"m/h/d"(与 UI 中文一致);逾期卡
  (mins ≤ 0)显示"现在",与"1 分后"可区分(原都显示"1m")。
- **状态栏设备名截断 + 卡片位置**:长蓝牙名("某人的 AirPods Pro Max (2nd generation)")截断到半宽
  预算;progress 加 `pos/total` 区分"今日已学完"(剩 0)与"本次完成"。
- **settings 打开时 cursor 重置到当前引擎**:Cancel-and-reopen 不再停留在上次位置(读起来像"高亮的
  是当前引擎"即使不是)。

### 显式不做(v3)

- `Vec<Overlay>` 栈重构 — 扁平 bool + 纪律修复已足够,重构成本不匹配收益。
- auto-detect reduced_motion — atuin 模型(config flag + env),终端不暴露 OS 偏好。
- 多步 undo — 破坏 FSRS 参数可信度。
- held-write grading(评分延迟提交) — 增加状态机复杂度,snapshot undo 已够。
- 2D 星座导航(graph_cursor → (group, member)) — P2,当前扁平导航可用,deferred。
- overlay scroll(ask/help/graph)— 小终端场景,bounded 内容 deferred。
- tachyonfx — v1 已决策不引入(版本钉子)。


## 全链路对抗审查后的修正(2026-07)

三个并行审查(TTS 链路 / 跨平台音频与发布链路 / multi-turn chat 与 undo)对照代码真相逐条核实后落地:

- **门策略硬化(fail-open 修复)**:`find_bound_output` 排除 ALSA 永在场虚拟 PCM(default/pulse/
  pipewire/…),它们让门永不关闭、声音被声音服务器路由到扬声器;macOS 上门只对蓝牙类设备开
  (宽松 needle 如 "air" 不再匹配 "MacBook Air扬声器")。`gate-test` 对被拒的名字近似匹配给出
  解释。向导候选同规则过滤。
- **播放设备与验证设备对齐**:`ensure_player` 用门验证过的设备全名开流(精确名优先,子串回退),
  不再用 needle 重新搜一遍;`play_audio` 播放前强制刷新门状态,关掉 1s 轮询窗口内"对死流报成功"
  的窗口。
- **Matcha lexicon 修复**:en_US-ljspeech tarball 不含 lexicon.txt,传该路径使 sherpa Validate 失败、
  create 返回 None,Matcha 装完即死。改为 lexicon: None(G2P 走 espeak-ng-data)。体积数字改为实测
  (73MB tarball,非 220MB)。
- **Kokoro 音色表修正**:kokoro-en-v0_19 是 11 音色(sid 0-10,af…bm_lewis);原代码只暴露不存在的
  "af_heart"(v1.0 音色),靠 unwrap_or(0) 兜底才工作。默认音色改为真实 sid-0 名 "af"。
- **解压原子化**:tarball 先解进 staging 再 rename 进位。原直接解包被 Ctrl-C 打断时所有文件在场但
  末一个截断,models_present(存在性检查)误报已装,合成永远失败且无恢复路径。
- **setup 重跑保配置**:init_config 从现有 Config 携带 speed/base_url/models/[a11y],向导只覆盖它
  问过的四个值。原实现整文件重写,reduced_motion 等手调项被静默重置。
- **undo 完整性**:save_card 传快照的 introduced(原硬编码 true,把撤销的新词永久标成"已引入",
  下会话按假复习调度);pos 用快照恢复(原 pos-1 在 load_current 跳词后错位)。
- **推导对话收口**:对话在 Esc 后保留(收起非丢弃,回复到达以 toast 提示,换卡才清);聊天输入
  独占键盘(Tab 不再在输入中途拉起命令菜单清空草稿);滚动 pin-to-bottom(新回复始终可见);
  system prompt 承诺的"真实含义"真的随消息发给模型(原来漏发,模型只能自己编一个"正确答案");
  重发历史截断到最近 12 条。
- **release 防 clobber**:tag 已存在且指向不同 commit 时硬停(触发路径含 workflow 文件而版本计数
  不含,workflow-only push 会用新 commit 重建同版本并覆盖已发布产物,击穿 Homebrew sha256);
  Linux 构建移到 ubuntu-24.04 runner + ubuntu:22.04 容器,22.04 runner 退役(2026-10)后仍保
  glibc 2.35 地板;`tuna --version` 编译期注入 CI tag(TUNA_VERSION),不再恒报 0.1.0。
- **cmdmenu enabled 即真相**:菜单行的可用态镜像基础键位门(词源/星座揭示后可用,防剧透;辨析
  需有当前卡),字母直达也走同一 enabled 门,不再出现"行是灰的但快捷键照发"或"行亮着但按了
  没反应"。

## AI 对话统一 + 回复朗读(2026-07,第二轮打磨)

- **一次性苏格拉底弹窗撤销,两种语境统一为多轮 AI 对话**:同一个 overlay、两个模式。
  Derive(新词未揭示):模型持有已核验词素与真实词义,红线是不直说,学习者自己推;
  Compare(揭示后):打开即由模型先手给出词根对照与引导提问,可继续追问。模式即语境,
  上下文按模式喂(compare 附易混/近义边),换模式换线程。CLI `tuna ask` 保留一次性输出。
  prompt 按 harness 纪律清理:只传事实与目标,红线只留"不直说词义"与"简短中文"两条硬约束。
- **回复朗读选 Kokoro 多语(fp32 v1_1)而非 MeloTTS**:回复是中文夹英文词素片段("spect"、
  "-ate"),MeloTTS 的纯词表前端会静默丢弃词表外英文,而这些恰是正在教的词;Kokoro 多语对
  词表外英文回退 espeak G2P。int8 包(~140MB)在 macOS arm64 上实测输出 NaN 静音,fp32
  (~350MB)实测正常,取 fp32。作为独立的"对话语音"下载(向导第④步,可选),不进学习引擎
  列表;`chat_speak` 持久化,对话内 Tab 切换;朗读一样走耳机门。`rule_fsts`(date-zh/
  number-zh)接顶层配置,数字日期先归一成中文;jieba `dict_dir` 必传(官方示例同,缺它
  中文分词降级)。
- **聊天滚动改为自算换行 + 底部钉住**:Paragraph 的 word-wrap 行数不可预测,估算误差
  直接表现为输入行滚出屏幕(多轮后必现)。改为 `wrap_line` 按显示宽度自行切行、渲染精确
  切片,滚动以"距底部行数"计,新回复与输入行永远可见。帮助 overlay 用同一套切片实现滚动。
- **选读焦点视觉**:REVERSED 箭头块改为 `▎` 左侧竖条 + SPEAK_BG 底色 wash + 行尾 ♪。
  竖条/音符/底色是结构线索,不单靠色相;未选中行保留双列缩进,光标移动不引起文本位移。
- **帮助面板补两套评分体系的语境**:FSRS 组标题注明"给当前这张卡打分",星火接线组注明
  "自动出现、给已学旧词加一次复习",并加"触发"行说明何时出现(与某已学词共享词根)、
  何时不出现(还没有已学同根词)——y/n 只在翻牌后生效的困惑源于此。

## 发版链路:版本号即发版决定(2026-07)

原方案两头都不对劲:主仓库每次 push 触及构建输入就跑四平台构建并自动发版
(patch 号 = 提交计数),tap 仓库每天 cron 轮询最新 release 重生成 formula。
代价是琐碎提交也烧 4 台 runner、release 列表被灌水、formula 最多滞后一天,
且"哪次提交算一个版本"没有人的判断。

现方案:
- **发版决定 = Cargo.toml 版本号提升**。version 是唯一真相源:升版本(并让
  Cargo.lock 跟上)推到 main,才触发构建;workflow 的 paths 只盯 Cargo.toml 与
  workflow 文件,普通代码推送零 CI。version job 秒级门控:tag 已存在于同一
  commit 视为重跑放行,存在于不同 commit 直接跳过(保护已发布产物的 sha256)。
  Cargo.lock 未跟上版本号时快速失败,不浪费 10 分钟构建。
- **`tuna --version` 回归 CARGO_PKG_VERSION**:版本号与 tag 天然一致,删掉
  TUNA_VERSION 编译期注入。自动版本时代止于 v0.1.29,自主版本从 0.2.0 起。
- **tap 事件驱动**:release 发布后 notify-tap job 向 homebrew-tuna 发
  repository_dispatch(带 tag),tap 的 sync-formula 立即按该 tag 重生成
  formula——formula 与 release 分钟级同步。每日 cron 降为每周自愈兜底
  (dispatch 因 token 缺失/过期丢失时补上),手动 workflow_dispatch 保留。
  跨仓库 dispatch 需要细粒度 PAT(TAP_DISPATCH_TOKEN,homebrew-tuna 的
  Contents 读写);secret 未配置时 notify 步骤打 notice 跳过,不红。

## 复习面板重排 + 聊天视口锚点(2026-07,第三轮打磨)

- **复习揭示(Phase B)不再复用 plain ECDICT 墙**:此前有精加工的复习卡也走纯 ECDICT 路径,
  四行 amber 释义糊成一片,且词素/例句全部不渲染,而 ↑↓ 选读光标仍指向这些看不见的例句
  (Space 会读出屏幕上不存在的句子)。新 review_reveal 按提取场景排版:答案先行(释义 amber
  引导 + ECDICT 释义按词性缩进成列,词性标签压暗),再一眼扫过推导脚手架(词素格 + 推导链),
  例句带选读标记、辨析收尾。无精加工的卡走升级版 plain_meaning(词性列 + 英释标签 + 同族)。
- **复习 Prompt 加提取上下文**:"第 N 次复习 · 距上次 X" 一行(reps + last_review),
  帮助定位回忆而不泄露答案。
- **聊天视口从"钉底部"改为锚点模型**:辨析 kickoff 的回复常超过弹窗高度,钉底部时用户只看到
  回复的尾巴和输入行,开头被卷出视口,表现为"回复被吞"。现在视口有两个锚点:回复到达锚定到
  该回复的第一行(阅读位),一打字/删字立即锚回底部(输入行永远不盲打);↑↓ 是相对锚点的
  偏移,渲染端按真实换行行数精确 clamp。同模式重开保留锚点与偏移(收起时到达的未读回复,
  重开即从头呈现)。切换对话模式时若上一模式回复仍在途,toast 告知丢弃,不再无声消失。

### 补记:"回复被吞"有两个叠加根因

视口锚点只解释了长回复的场景。真机截图显示回复气泡本身是空的:`chat_multi` 只读
`content` 字段,而推理模型(deepseek-v4-pro)的思维链从同一个 max_tokens 预算里扣,
500 的上限在辨析类问题上整个被思维链吃光,`content` 以空字符串"成功"返回,被静默渲染成
一个空 ✦ 气泡。修法三层:max_tokens 全线放开到 8192(回复本身仍由 system prompt 约束简短,
余量给思维);`chat_multi`/`chat` 对空 content 按 finish_reason 报错而不是返回空串
(空气泡是最坏的失败形态,错误 toast 是诚实的);渲染端对"markdown 解析出零行但原文非空"
回退为原文纯文本,防解析器吞掉不支持的构造。

## 语法融入:支撑知识,不是第二门课程(2026-07)

学习者可能零语法基础,例句和 AI 对话随处会撞上语法墙("为什么不能说 lean the wall")。
评估过独立语法课程模块(FSRS 语法卡),否决:需要烘焙整套课程资产,等于再造一个
enrichment 管线,且脱离词汇场景的语法卡违背"在用的地方学"。语法以三个咬合点融入现有链路:

- **例句语法对话(ChatMode::Grammar)**:`a` 的语义升级为"指哪问哪"——新词未揭示=推导,
  选中单词=辨析,↑↓ 选中某个例句=讲这句。语法对话是讲解式而非苏格拉底式:先一句话给
  句子骨架,再讲目标词的角色,追问直接答(语法是脚手架知识,不是要学习者自己推出的谜题;
  真机对话里学习者问"为什么不能直接说 X"被反问,正是这个错位)。对话身份含例句序号,
  指向另一句自动开新线程。keybar 的 `a` 标签随光标变(辨析/析这句)。
- **离线语法速查(`x`)**:一屏半的生存底座,大白话解码 tuna 自己暴露的语法面——释义前的
  词性缩写(n./vt./prep.)、句子骨架、为什么需要介词、看长句的顺序。静态内容,离线可用,
  任意阶段可开(参考资料无剧透)。不是课程,是底座。
- **推导/辨析 prompt 分工修正**:苏格拉底红线收窄到只管"词义"——词义之外的问题(语法、
  用法)直接用大白话讲清楚,并声明学习者几乎没有语法基础、术语当场解释。
