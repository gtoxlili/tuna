# 当前状态与待办

本文档记录已完成、已知缺口、刻意押后项、待决问题。接新需求前先读这里,避免重复劳动或
误把"刻意没做"当"漏做"。

## 已完成

### 基础里程碑(M0–M5)

- **M0** 耳机门 + CoreAudio 枚举 ✓ `probe` / `gate-test` 真机验证通过(默认输出=扬声器时
  chime 只进 AirPods)。
- **M1** 数据管线(ECDICT → 考研牌组 → FSRS/SQLite)✓
- **M2** 复习循环 + Ratatui 界面(拆·联·验 + 耳机门指示 + FSRS 间隔预览)✓
- **M3** DeepSeek 词条精加工(词素/推导链/诚实词源/例句)+ 词根图边 + 拆·联 界面 ✓
- **M4** Kokoro TTS + 耳机门播放(`Space` 发音)✓ 后升级为 sherpa-onnx 多引擎单二进制
  (Kokoro/Matcha/Piper 统一 OfflineTts API,砍掉 Python/uv/espeak)。
- **M5** 打磨:词根图谱浮现(「你学过 X,同根」)+ 苏格拉底辨析(`a`)✓

### 打磨阶段(P0–P7)

- **P0** morpheme 节点脊柱 + 杀 `known_anchors` 锚点谎言 ✓
- **P1a** Wiktionary 覆盖率镜子(grounded etymology pipeline)✓
- **P1b** S4 关笼 LLM 烤制(canonical morpheme nodes + caged LLM narration)✓
- **P1c** 位置感知聚类 id(保留连字符作位置编码)✓ `4518/4801` 词 grounded enrichment
- **P2** 星火接线 earned-edge 引擎(回忆一个已学同根词,给它真 refresh)✓
- **P3** guess-eval(你的推导猜测成为 live 苏格拉底通道)✓
- **P4** 签名动画(earned-strike arc + 可点 Wiktionary 引文,`w`)✓
- **首启向导** 三步(绑耳机/密钥/下模型)✓
- **P5** 星座 root-family(`g`):同根已学词 + 只差一个词根的前沿暗星 ✓
- **P6** 同源合并:Wiktionary 碎片折回最简词根(spect 家族 2→6 词),gloss-gated ✓
- **P7** 纯 Rust 单二进制:sherpa-onnx 静态链接 C++ 库,多引擎统一 OfflineTts API,砍 Python/uv/espeak ✓
- **跨平台移植**:TTS 换 sherpa-onnx 解除 ort 的 Windows MSVC-only 阻塞;CoreAudio 依赖收进
  `cfg(target_os="macos")` 门;`trait AudioProbe` 抽象 macOS CoreAudio / Linux ALSA / Windows WASAPI
  三后端。门语义在非 macOS 平台降级为按名字绑定(fail-closed 原则三平台一致)✓
- **多引擎 TTS 切换**:首启向导第③步选引擎(Kokoro/Matcha/Piper),运行时按 `s` 热切换 overlay,
  写 config + drop warm session 重载 ✓
- **方向键选读交互**:揭示后 `↑↓` 在单词/例句间切换朗读目标,`Space` 朗读选中项;星座 overlay 内
  `↑↓` 导航 `Space` 朗读选中词 ✓
- **生产 release profile**:LTO fat / codegen-units=1 / strip=symbols / panic=unwind + 平台 target flags ✓
- **审计与死代码清理**:`enrich.py`(脚枪)/ `synth.py` / `known_anchors` prompt 规则 / morpheme 死列
  (variants/gloss_en/src_lang/etymon/citation/specificity)全部删除 ✓

当前 HEAD 在 `build/mvp`,约 28 个提交。

## 已知缺口(诚实注记,非 bug)

- **needs-review 5%(~241 词)**:Wiktionary templates present 但 parser 漏 ≥2 词素。走 reconciliation
  agent / 人工审,不静默猜测。
- **`port` 家族英文 `port` 与 Latin `portō` 分裂**:聚类完整性缺口,已知未连。
- **dictate↔predict 跨类未连**:single-root(Latin `dictātum`)与 cited-affix(English `dict`)未结构化
  连接。留给 cited cognate 边作链接源,不做 hacky Latin-stemmer。
- **`ky` 标签 4801 词 vs 官方 ~5500 大纲**:ECDICT `ky` 标签与官方大纲有差,列为精修项。
- **4519/4521 烤制差 2 词**:全量 narrate 重跑后最终 4519 < 4521,差 2 词未明(可能限流跳过未补)。

## 刻意押后(数据依赖,不是漏做)

这些都需要**真实复习数据**才能做,新装零数据;`review_log` 表已就位正在记录,等攒数据再训。
提前做只会得到用假数据训的废模型(等于建无用拐杖)。

- **个人 FSRS 权重离线拟合**:需真实复习历史。
- **LightGBM 冷启动难度预测器**:从内容特征(词素数/已掌握词根/词频/词长/词源置信度)+ 历史性能
  预测"黏词",用于排序引入顺序 + 给会黏的词加脚手架。同上需历史。
- 当前用词频序 `dict.priority` 代替冷启动排序。

## 待决问题(需用户明示)

### sherpa 引擎音色 / 自然度

三引擎音色特征各异,未 A/B 验证:Kokoro(风格向量 TTS,英文女声 af_heart)、Matcha(条件流匹配,
LJSpeech 女声)、Piper(VITS 社区多音色,默认 Lessac 女声)。发音是核心交互,无法替用户
判断音质,需用户真机实听拍板选哪个引擎。可测的管线都健康(`tuna synth` 烟测通过,运行时按 `s`
可热切换引擎重听对比)。

## Backlog(未排期)

- **tachyonfx 揭示动画**:版本坑仍在(钉 ratatui 0.29);在 baked 数据上做逐步推导揭示,营造
  "在你眼前推"的活感。当前用自制 anim clock 替代。
- **真题语料**(考研):接入真题例句。
- **学习仪表盘**:每日进度 / 到期预测。
- **`morpheme_mastery` 表**:词素级掌握度。
- **Socratic 辨析按真实 lapse 挑战场**:而非泛型 confusables。
- **希腊词根罗马化**(cited-1hop)。
- **`-sid-` 这类中缀词根的合并**:需真正的形态分析,暂缓。
- **Etymonline 第二源交叉核验**:v1 用 Wiktionary-only,Etymonline deferred。
- **视觉美化 pass**:排版 / 留白 / 层次系统性打磨。
