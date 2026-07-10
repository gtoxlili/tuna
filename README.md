# tuna

一个终端里的词根推导学习工具，为考研英语（英语一）设计。在办公室里通过绑定的蓝牙耳机、用推导而不是死记的方式学词汇。

## 它做什么

方法叫拆·联·验（Decompose · Link · Retrieve）：把词拆成词素，把新词接到你已掌握的词根图上，再用 FSRS 间隔重复做生成式提取。发音只走绑定的耳机，连不上就静默，全程不用出声。

## 特点

- **词根推导而非死记**。每个词都拆成词素（前缀、词根、后缀），你先自己推一次再看答案。记住的是词怎么来的，不是孤立的拼写。
- **耳机门**。音频流直接开在你绑定的蓝牙耳机上，不指向笔记本扬声器。耳机不在场就静音，不会回退到扬声器。办公室里不会漏音。
- **词源诚实**。词素来自 Wiktionary 的确定性解析，每根可回溯到引用版本。LLM 只负责翻译和叙述已经核验过的词素，结构上加不进也改不了任何词根。按 `w` 打开该词的 Wiktionary 词源页。
- **星火接线**。新词揭示后，系统问你"词根 act 你在哪个已学的词里见过"。你在脑子里回忆，翻牌，自己打分。回忆成功就给那个旧词记一次真实的 FSRS 复习。词和词之间的边是你自己回忆出来的，不是机器替你画的。
- **星座**。按 `g` 看当前词的词根家族。你亲手学过的同根词会发光（绿色表示记忆已稳固，琥珀色表示还新鲜），暗星是只差一个词根就能推导出来的前沿。只画真实共享词素的边，语法后缀不算。
- **AI 对话，指哪问哪**。新词揭示前按 `a` 和 AI 多轮对话着拆词：你说你看到了哪些词素，它引导你自己推出词义，绝不直接说答案；揭示后选中单词按 `a` 是易混词辨析，用 `↑↓` 选中某个例句再按 `a`，AI 会用大白话讲这个句子的结构和目标词在里面的角色——语法不是谜题，直接讲清楚。回复可以朗读出来（中英混合语音，对话内 `Tab` 切换，同样只走绑定耳机）。需要 DeepSeek 密钥，学习本身离线可用。
- **语法速查**。按 `x` 打开离线的语法底座：词性缩写（n./vt./prep. 到底是什么）、句子骨架、为什么需要介词、看长句的顺序，全部大白话，正好解码释义前的那些缩写。
- **本地发音**。sherpa-onnx 静态链接 C++ 库，Kokoro、Matcha、Piper 三个引擎可切换，整条文本到波形的链路都在二进制里。不用 Python，不用系统 espeak。首次合成约 0.6 秒，之后落 WAV 缓存即取即播。
- **单二进制自包含**。4801 词的词典和精加工数据都编进了二进制，不用下载任何数据文件。`cargo install` 装完就能跑。

## 安装

### macOS（Homebrew）

```bash
brew tap gtoxlili/tuna
brew trust gtoxlili/tuna
brew install gtoxlili/tuna/tuna
```

装出来的命令叫 `tuna`。注意必须用全限定名 `gtoxlili/tuna/tuna`：homebrew-core 里有一个同名的 cask（`tuna`，一个无关的 Mac 启动器），直接 `brew install tuna` 会装到那个 cask 上。Homebrew 6.0 起第三方 tap 需要显式信任（`brew trust`）才会加载；低于 6.0 的版本省略 `brew trust` 这一步。

二进制覆盖 Apple Silicon（M1 及以上）和 Intel Mac。

### cargo

```bash
cargo install --path .
```

需要本地有 Rust 工具链。从源码编译，sherpa-onnx 的预编译库会自动下载。

### 直接下载

到 [Releases 页面](https://github.com/gtoxlili/tuna/releases) 下载对应平台的压缩包，解压后把 `tuna` 放到 PATH 里。提供 macOS arm64、macOS x86_64、Linux x86_64、Windows x86_64 四个预编译版本。

Linux 版按 x86-64-v3（约 2015 年后的 CPU，需要 AVX2）编译，更老的机器请用 cargo 从源码装。另外 Linux 上耳机按设备名绑定，`default`、`pipewire` 这类永远在场的虚拟设备不能作为绑定目标（门无法关闭），蓝牙耳机需要以真实的 ALSA 设备出现。

## 快速开始

```bash
tuna                       # 首次运行进入设置向导，之后开始学习
```

首次运行是四步向导：从你连着的蓝牙设备里选一副耳机绑定，粘贴 DeepSeek 密钥（可选，学习本身离线可用），下载发音模型，最后可选下载 AI 对话朗读用的中英混合语音（约 350MB）。之后在 `~/.tuna/` 建好一切：

| 路径 | 内容 |
|---|---|
| `~/.tuna/config.toml` | 配置（密钥、绑定耳机、TTS 引擎、音色） |
| `~/.tuna/tuna.db` | 牌组（4801 词）和你的复习状态 |
| `~/.tuna/cache/audio/` | 发音缓存 |
| `~/.tuna/tts/` | 发音模型 |

用 `$TUNA_HOME` 可以改根目录。苏格拉底辨析需要 DeepSeek 密钥，在 `~/.tuna/config.toml` 里填，或设 `$DEEPSEEK_API_KEY`。

## 命令

| 命令 | 作用 |
|---|---|
| `tuna`（或 `tuna study`） | 开始学习 |
| `tuna ask <word>` | 苏格拉底式辨析该词与易混词 |
| `tuna deck-info` | 牌组统计和频率序队列 |
| `tuna probe` | 列出音频设备（UID、传输、输出流） |
| `tuna gate-test [needle]` | 播测试音，只走绑定耳机，不在场则静默 |
| `tuna setup` | 重跑设置向导（重绑耳机、重设密钥、下载发音模型与对话语音） |

学习时的按键：`Tab` 打开命令菜单，`Enter` 揭示答案，`Space` 发音，`a` 问 AI（揭示前推导、选中例句讲语法、其余辨析），`x` 语法速查，`w` 词源，`g` 星座，`s` 设置，`?` 帮助，`u` 撤销上一次评分，`Esc` 退出。

## 维护者

重建内嵌资产（普通用户不用关心）：

```bash
tuna build-deck            # ECDICT(data/stardict.db) → data/tuna.db
tuna export-deck           # data/tuna.db → assets/deck.jsonl（提交进仓库）
uv run scripts/bake.py     # Wiktionary 词源接地 → data/etym_cache.jsonl
uv run scripts/narrate.py  # 词根聚类 + 受控 LLM → assets/{morphemes,enrichment}.jsonl（提交）
```

精加工是离线烤制、结果提交进仓库，用户零 LLM 成本。`bake.py` 抓 Wiktionary 模板做确定性词源解析，`narrate.py` 只让 LLM 翻译和串词已经验证的词素，禁止编造词根。

发版：把 `Cargo.toml` 的 `version` 升一号，跑一次 `cargo build` 让 `Cargo.lock` 跟上，推到 main。只有这样的推送才会触发四平台构建和 GitHub Release，发布完成后自动通知 homebrew-tuna 更新 formula；普通代码推送不占用任何 CI。

## 许可

GPL-3.0-or-later。联系：gtoxlili@outlook.com
