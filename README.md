# tuna

一个终端里的**词根推导终端** —— 为考研英语（英语一）设计，让你在办公室静默地、通过绑定的蓝牙耳机、以推导而非死记硬背的方式学词汇。

> 词汇不是要*存储*的事实，是要*推导*的公式。

方法叫 **拆·联·验**（Decompose · Link · Retrieve）：先把词拆成词素、把新词接到你已掌握的词根图上，再用 FSRS 间隔重复做生成式提取。发音只走绑定的耳机（连不上就静默），全程无需出声。

设计评审：<https://claude.ai/code/artifact/a249c729-f9b6-46e6-a0fd-2c85e42ba073>

## 技术栈

Rust · [Ratatui](https://ratatui.rs)（TUI）· [cpal](https://github.com/RustAudio/cpal)/[rodio](https://github.com/RustAudio/rodio)（耳机门，零 CGO 绑定指定设备）· [rusqlite](https://github.com/rusqlite/rusqlite) · [rs-fsrs](https://github.com/open-spaced-repetition/rs-fsrs) · [ECDICT](https://github.com/skywind3000/ECDICT)（离线词典，已内嵌）· DeepSeek（辨析）· Kokoro（本地 TTS，懒加载）。

## 快速开始

词典与精加工都**编进了二进制**——无需下载 ECDICT、无需任何数据文件。

```bash
cargo install --path .     # 或 cargo run --
tuna                       # 首次运行:三步设置向导,然后开始学习
```

首次运行是一个**三步向导**:① 从你连着的蓝牙设备里选一副耳机绑定(只有它连着时才发声)· ② 粘贴 DeepSeek 密钥(可选,学习本身离线可用)· ③ 现在或稍后下载 Kokoro 发音模型。之后在 `~/.tuna/` 建好一切:

| 路径 | 内容 |
|---|---|
| `~/.tuna/config.toml` | 配置(DeepSeek 密钥、绑定耳机、音色) |
| `~/.tuna/tuna.db` | 牌组(4801 词)+ 你的复习状态 |
| `~/.tuna/cache/audio/` | 发音缓存 |
| `~/.tuna/tts/` | Kokoro 模型 |

学习本身**离线可用、无需密钥**。想启用苏格拉底辨析,在 `~/.tuna/config.toml` 里填 DeepSeek 密钥(或设 `$DEEPSEEK_API_KEY`)。用 `$TUNA_HOME` 可改根目录。

## 命令

| 命令 | 作用 |
|---|---|
| `tuna`（或 `tuna study`） | 开始学习(`Enter` 揭示 · `Space` 发音 · `a` 辨析) |
| `tuna ask <word>` | 苏格拉底式辨析该词与易混/近义词 |
| `tuna deck-info` | 牌组统计 + 频率序队列 |
| `tuna probe` | 列出 CoreAudio 设备(UID/传输/输出流)——耳机门的事实来源 |
| `tuna gate-test [needle]` | 播测试音,**只**走绑定耳机;不在场则静默 |

## 发音（Kokoro TTS · 懒加载）

需要 [`uv`](https://docs.astral.sh/uv/) 与 Kokoro 模型(约 92MB + 28MB,int8)放到 `~/.tuna/tts/`:

```bash
curl -L -o ~/.tuna/tts/kokoro-v1.0.int8.onnx \
  https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.int8.onnx
curl -L -o ~/.tuna/tts/voices-v1.0.bin \
  https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
```

**无需预合成。** 按 `Space`,若该词未缓存就**当场合成**:`~/.tuna/synth.py --server` 是常驻热进程,模型只加载一次,首次 ~6s(有 spinner)、之后 ~300ms,合完落缓存。`uv run` 自建环境(含 espeak-ng,无需系统安装)。

## 维护者（重建内嵌资产）

```bash
tuna build-deck        # ECDICT(data/stardict.db) → data/tuna.db
tuna export-deck       # data/tuna.db → assets/deck.jsonl(提交进仓库)
uv run scripts/enrich.py   # DeepSeek 精加工 → assets/enrichment.jsonl(提交)
```

## 耳机门

办公室静默是这个产品的情感中心。tuna 把播放流**直接开在绑定的蓝牙耳机上**——手里从来没有一条指向笔记本扬声器的流，所以漏音在物理上不可能，而不是靠一个 `if` 拦着。耳机不在场 ⇒ 零音频，绝不回退到扬声器。绑定按设备 UID（跨重连稳定、内嵌 MAC），因为 AirPods 会以同名的输入/输出两个设备出现，只有 UID + 输出流数能区分。

## 状态

- **M0** 耳机门 + CoreAudio 枚举 ✓
- **M1** 数据管线（ECDICT → 考研牌组 → FSRS/SQLite）✓
- **M2** 复习循环 + Ratatui 界面（拆·联·验 + 耳机门指示 + FSRS 间隔预览）✓
- **M3** DeepSeek 词条精加工（词素/推导链/诚实词源/例句）+ 词根图边 + 拆·联 界面 ✓
- **M4** Kokoro TTS 离线预合成 + 耳机门播放（`Space` 发音）✓
- **M5** 打磨：词根图谱浮现（「你学过 X，同根」）+ 苏格拉底辨析（`a`）✓
  - backlog：tachyonfx 揭示动画、真题语料、个人 FSRS 权重离线拟合、学习仪表盘

## 许可

GPLv3
