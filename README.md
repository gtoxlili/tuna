# tuna

一个终端里的**词根推导终端** —— 为考研英语（英语一）设计，让你在办公室静默地、通过绑定的蓝牙耳机、以推导而非死记硬背的方式学词汇。

> 词汇不是要*存储*的事实，是要*推导*的公式。

方法叫 **拆·联·验**（Decompose · Link · Retrieve）：先把词拆成词素、把新词接到你已掌握的词根图上，再用 FSRS 间隔重复做生成式提取。发音只走绑定的耳机（连不上就静默），全程无需出声。

设计评审：<https://claude.ai/code/artifact/a249c729-f9b6-46e6-a0fd-2c85e42ba073>

## 技术栈

Rust · [Ratatui](https://ratatui.rs)（TUI）· tokio · [cpal](https://github.com/RustAudio/cpal)/[rodio](https://github.com/RustAudio/rodio)（耳机门，零 CGO 绑定指定设备）· [rusqlite](https://github.com/rusqlite/rusqlite) · [rs-fsrs](https://github.com/open-spaced-repetition/rs-fsrs) · [ECDICT](https://github.com/skywind3000/ECDICT)（离线词典）· DeepSeek（词条精加工）· Kokoro（本地 TTS 预合成）。

## 快速开始

```bash
# 1. 下载 ECDICT 词典（离线词源/音标/词频/变形，约 216MB 压缩）
mkdir -p data && cd data
curl -L -o ecdict-sqlite-28.zip \
  https://github.com/skywind3000/ECDICT/releases/download/1.0.28/ecdict-sqlite-28.zip
unzip ecdict-sqlite-28.zip     # → data/stardict.db
cd ..

# 2. 构建考研牌组（筛出 ky/考研 标签词，建立 FSRS 卡片）
cargo run -- build-deck        # → data/tuna.db（约 4800 词）
cargo run -- deck-info         # 查看统计与队列
```

`data/` 与 `cache/` 是本地产物，已被 gitignore；`tuna.db` 随时可由 `build-deck` 重建。

## 命令

| 命令 | 作用 |
|---|---|
| `tuna probe` | 列出所有 CoreAudio 设备（UID / 传输类型 / 输出流数）——耳机门的事实来源 |
| `tuna gate-test [needle]` | 播一段测试音，**只**走绑定的耳机；耳机不在场则静默（fail-closed） |
| `tuna build-deck` | 从 ECDICT 构建考研牌组 |
| `tuna deck-info` | 牌组统计 + 频率序队列预览 |
| `tuna enrich --limit N` | 用 DeepSeek 精加工 N 个词（词素/推导/图边/例句），存入牌组 |
| `tuna synth --limit N` | 用 Kokoro 离线预合成前 N 个词（+例句）的发音到 `cache/audio` |
| `tuna ask <word>` | 苏格拉底式辨析该词与易混/近义词（DeepSeek chat 模型） |
| `tuna`（或 `tuna study`） | 开始学习会话（`Space` 发音 · `a` 辨析 · `Enter` 揭示） |

DeepSeek 密钥放在 `tuna.toml`（已 gitignore）或 `$DEEPSEEK_API_KEY`。精加工是离线批处理：夜里把明天要见的词跑一遍，桌前零延迟、全静默。系统提示是 byte-stable 前缀，命中 DeepSeek 的 prompt-cache，整轮成本约几美元。

## 发音（Kokoro TTS · 懒加载）

需要 [`uv`](https://docs.astral.sh/uv/)。只需下载 Kokoro 模型（约 92MB + 28MB，int8）：

```bash
mkdir -p data/tts/models && cd data/tts/models
curl -L -O https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.int8.onnx
curl -L -O https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
cd ../../..
```

**无需预合成。** 学习时按 `Space`，若该词未缓存就**当场合成**：`sidecar/synth.py --server` 是一个常驻热进程，模型只加载一次并驻留，首次 ~6s（有 spinner）、之后 ~300ms，合完落缓存、下次直接命中。发音只走绑定的耳机，耳机不在场则静默。缓存按 `hash(文本+音色+语速)` 内容寻址。

`tuna synth --limit N` 仍可选——夜里提前把前 N 个词灌进缓存，纯粹图个桌前零延迟。`uv run` 会自建环境（含 espeak-ng，无需系统安装）。

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
