# 架构

tuna 是一个**同步 Ratatui TUI** 应用,所有状态在一个 SQLite 文件里,音频走独立的设备绑定
播放器,LLM / TTS 这类慢操作通过后台线程 + channel 接回主循环。本文档描述模块边界、数据
模型、运行时模型与关键不变式。

## 模块图

```
src/
├── main.rs            CLI 入口 + clap 命令分发 + bootstrap(首启建 ~/.tuna)
├── paths.rs           ~/.tuna 路径解析($TUNA_HOME 覆盖);所有文件位置的单一来源
├── config.rs          ~/.tuna/config.toml 读写;DEEPSEEK_API_KEY 环境覆盖
├── assets.rs          include_str!/include_bytes! 把 deck/enrichment 资产编进二进制
├── setup.rs           首启三步向导(绑耳机/密钥/下模型)+ 阻塞式带进度条下载
├── audio/
│   ├── probe.rs       trait AudioProbe 跨平台抽象;macOS 走 CoreAudio,Linux/Windows 走 cpal
│   ├── coreaudio.rs   CoreAudio HAL 枚举(UID/transport/out-streams);cfg(target_os="macos") 门
│   ├── player.rs      RoutedPlayer:把流开在指定 cpal 设备上;drop = 即时静音
│   ├── tts/           sherpa-onnx 多引擎 TTS(Kokoro/Matcha/Piper 统一 OfflineTts API)
│   │   ├── mod.rs     TtsEngineKind / TtsEngine trait(静态描述符)/ TtsConfig / SynthSession trait
│   │   ├── session.rs SherpaSession:暖推理 OfflineTts 实例,gen 保留字避开用 gen_cfg
│   │   ├── kokoro.rs  Kokoro 引擎描述符(风格向量 TTS,sid 选音色)
│   │   ├── matcha.rs  Matcha 引擎描述符(条件流匹配 + HiFiGAN vocoder)
│   │   └── piper.rs   Piper 引擎描述符(VITS 社区多音色)
│   └── mod.rs
├── data/
│   ├── schema.rs      SQLite schema(8 张表)+ PRAGMA;单一 SCHEMA 常量
│   ├── deck.rs        Deck:字典/FSRS card/词素图/同源合并 的全部查询与写入
│   ├── scheduler.rs   rs-fsrs 包装;grade / preview;FSRS 定位为"镜子"只报告到期
│   └── mod.rs
├── llm/
│   ├── mod.rs         DeepSeek OpenAI 兼容 blocking 客户端;chat_json / chat_text
│   ├── enrich.rs      词条精加工 schema(Enrichment/Morpheme/GraphEdge/Example)+ enrich_word
│   ├── socratic.rs    苏格拉底辨析 + guess-eval(点评用户的推导猜测)
│   └── mod.rs
└── ui/
    ├── app.rs         App 状态机:Stage/Strike/Ask/Gate;on_key 事件路由;后台轮询
    ├── view.rs        纯渲染:render() + render_ask/constellation/strike 等子视图
    ├── theme.rs       调色板(墨底/phosphor-teal/amber)
    └── mod.rs         run()(同步事件循环)+ preview()(TestBackend 无 TTY 验证)
```

`scripts/{bake,narrate}.py` 是**离线资产烘焙管线**(维护者跑,产物提交进 `assets/`),
不进二进制运行路径;详见 [data-pipeline.md](./data-pipeline.md)。

## 数据模型

单一 SQLite 文件(`~/.tuna/tuna.db`),WAL 模式 + 外键开启。schema 在
[src/data/schema.rs](../src/data/schema.rs) 的 `SCHEMA` 常量里,8 张表:

| 表 | 持有 | 备注 |
|---|---|---|
| `dict` | 考研词典事实(ECDICT,2.2M,内嵌) | `priority` = 词频序(引入顺序) |
| `card` | 每个 word 的 FSRS 状态 | `introduced` 标记是否过了 Phase A(拆·联) |
| `review_log` | 每次评分 | 给仪表盘 + 未来个人权重拟合用 |
| `meta` | key-value | |
| `enrichment` | DeepSeek 精加工(整段 JSON verbatim + 少量过滤列) | `etymology_confidence`:solid/folk/mnemonic |
| `edge` | **纯 word↔word 成对边**(synonym/antonym/confusable) | cognate_root **永不**存这里 |
| `morpheme` | 词素图脊柱(一等节点) | `bond`:1=真推导桥(root/prefix),0=语法后缀(-ment/-tion),永不作锚点 |
| `word_morpheme` | word ↔ morpheme_id 的 M:N | cognate 关系 = 两个 word 共享 morpheme_id 的 JOIN |

### 关键关系:同根(cognate)是算出来的,不是存的

两个词同根 = 它们在 `word_morpheme` 里共享同一个 `morpheme_id`。`Deck::learned_siblings` /
`anchor_candidates` / `constellation` 都走这个 JOIN,且都带 `m.bond = 1` 过滤(语法后缀不算
推导之桥)。这是为什么 `edge` 表里没有 cognate 类型，它派生，不存储。

`morpheme.bond` 是一等字段,因为 `canonicalize_cognates()` 会把 `-ment` 规范化成 `ment`,
如果靠 hyphen 区分 suffix/root 会让后缀被静默升权到 root。集中字段杜绝该回归。

## 运行时模型

**同步 Ratatui**(不用 async runtime)。主循环在 [src/ui/mod.rs](../src/ui/mod.rs) `run()`:

- 事件循环:spinner 激活时 80ms 重绘,否则 250ms idle-poll。
- 耳机门:每 ~1s `poll_gate()` 重查绑定设备在场状态(`App::gate: GateStatus`)。
- 后台工作(LLM 辨析、TTS 合成)在独立线程跑,结果经 channel(`ask_rx` / `tts_rx`)回主循环;
  `poll_async()` 每轮拉一次。
- 音频播放:`RoutedPlayer` 持有一条开在绑定设备上的流;`drop` 即静音。

### UI 状态机

`App`( [src/ui/app.rs](../src/ui/app.rs) )持有几个独立状态机:

- **Stage**:`Prompt`(Phase A 拆·联,新词) / `Revealed`(Phase B 验,复习)。`Enter` 揭示,`Space` 发音。
- **Strike**(P2 星火接线):`Prompt` / `Flipped` / `Idle`。新词揭示后弹"词根 X 你在哪个已学的词里见过?",
  用户脑内回忆 → `Space` 翻牌 → 显示 FSRS 挑的最佳老词 → y/n 给那个老词记一次真复习。
  Strike 期间**阻塞新词评分**;Prompt 阶段隐藏 siblings 列表防剧透。节点只在被回忆时愈合。
- **Ask**(P3 guess-eval):`Idle` / `Pending` / `Answer` / `Failed`。`a` 键在有 typed guess 时,
  把用户推导猜测发 DeepSeek 做苏格拉底式点评(LLM 提引导性问题,不给直接判决)。
- **Gate**:`Open` / `Closed`。Closed 时零音频。

### 键位(学习界面)

| 键 | 作用 |
|---|---|
| `Enter` | 揭示(Phase A → B) |
| `Space` | 发音(只走绑定耳机;未缓存则当场合成,首次含图优化 ~0.6s release) |
| `↑↓` | 选读(揭示后切换朗读目标:单词本身 / 例句;wraparound) |
| `a` | 苏格拉底辨析 / guess-eval(需 DeepSeek 密钥) |
| `w` | 打开该词 Wiktionary 词源页 |
| `g` | 星座:当前词的词根家族(同根已学词 + 只差一个词根的前沿暗星);overlay 内 `↑↓` 导航 `Space` 朗读 |
| `s` | 设置:运行时切换 TTS 引擎 overlay(Kokoro/Matcha/Piper) |

## CLI 命令面

[src/main.rs](../src/main.rs) `Cmd` 枚举。用户可见命令都会先 `ensure_ready()`(首启 bootstrap):

| 命令 | 可见 | 作用 |
|---|---|---|
| `tuna`(或 `tuna study`) | ✓ | 学习会话 |
| `tuna ask <word>` | ✓ | 苏格拉底辨析(需密钥) |
| `tuna deck-info` | ✓ | 牌组统计 + 频率序队列 |
| `tuna probe` | ✓ | 列 CoreAudio 设备(UID/transport/out-streams) |
| `tuna gate-test [needle]` | ✓ | 测试音只走绑定耳机;不在场静默 |
| `tuna setup` | ✓ | 重跑设置向导(重绑耳机/重设密钥/下模型) |
| `tuna build-deck` | 隐藏 | 维护者:从 ECDICT 建开发库 |
| `tuna export-deck` | 隐藏 | 维护者:导出 `assets/deck.jsonl` |
| `tuna enrich` | 隐藏 | 维护者:DeepSeek 精加工进开发库 |
| `tuna render-preview` | 隐藏 | dev:TestBackend 渲染验证(无 TTY) |
| `tuna synth` | 隐藏 | dev:合成 WAV 不播放,验证 sherpa-onnx 管线 |

`Probe` 和 `GateTest` **不**调 `ensure_ready()`,无需 `~/.tuna` 已初始化即可跑。

## 关键不变式

这几条是系统属性,改相关代码前必须守住(完整理由见 [conventions.md](./conventions.md)):

- **耳机门是物理保证,不是 if 拦截**:`RoutedPlayer` 把流直接开在绑定的 cpal 设备上,不指向
  系统默认输出的流,所以漏音在物理上不可能。耳机不在场 → `find_output_device` 返回 `None` →
  fail-closed 静音,不回退扬声器。绑定契约跨平台有差异:macOS 按设备 UID(跨重连稳定、内嵌 MAC,
  解决 AirPods 同名输入/输出掷硬币问题);Linux/Windows 因 cpal 0.17 不暴露稳定 UID/transport,
  回退按显示名绑定，setup 向导显式告警 ALSA/WASAPI 名字可能随重启漂移,需重跑 setup 重绑。
- **单二进制自包含**:`assets.rs` 把 4801 词词典(2.2M,`DECK`)+ 精加工(`ENRICHMENT`)`include_str!`
  进二进制;morpheme 脊柱在运行时从 enrichment 派生(`normalize_morpheme`),`assets/morphemes.jsonl`
  虽提交进仓库但不被加载(只作人类可审 spine)。TTS 模型(63–320MB,随引擎)首启向导同步下载 +
  进度条。用户路径无需 ECDICT、无需 Python、无需系统 espeak(sherpa 预编译包内嵌 espeak-ng-data)。
- **跨平台三后端**:macOS 走 CoreAudio HAL(`coreaudio-sys` + `core-foundation`,收进
  `cfg(target_os="macos")` 门),Linux 走 cpal ALSA,Windows 走 cpal WASAPI。`trait AudioProbe`
  抽象枚举,`current_probe()` 按目标挑后端;无匹配目标则 `compile_error!`。门语义在非 macOS 平台
  降级为按名字绑定(见上条),但 fail-closed 原则三平台一致。
- **panic = "unwind"**:TUI 靠 unwinding 在 panic 时恢复终端(Drop guard 还原终端模式);`abort` 会
  跳过 Drop 把终端弄烂。这是 release profile 里显式保留 unwind 的理由。

## 配置

`~/.tuna/config.toml`(模板在 [src/config.rs](../src/config.rs) `TEMPLATE`):

```toml
[deepseek]
api_key = ""                                    # 或 $DEEPSEEK_API_KEY 覆盖
base_url = "https://api.deepseek.com"
enrich_model = "deepseek-v4-flash"              # 精加工
chat_model = "deepseek-v4-pro"                  # 辨析/guess-eval

[gate]
needle = "airpods"                              # 绑定耳机的名字子串

[tts]
engine = "kokoro"                                # kokoro | matcha | piper（运行时按 s 切换）
voice = "af_heart"
speed = 1.0
```

学习本身**离线可用、无需密钥**。辨析 / guess-eval 才需要 DeepSeek 密钥(`require_key()` 会 bail)。
