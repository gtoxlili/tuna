# 数据烘焙管线

tuna 的词素 / 推导 / 例句是**静态内容**——对所有人都一样,所以一次烘焙、提交进 `assets/`、
内嵌进二进制。用户路径**零 LLM 成本、零网络**(首启只下 TTS 模型)。这条管线由维护者跑,
产物进 git。本文档讲清两阶段管线、产物归属、重建命令。

## 两条路径

| 路径 | 谁跑 | 输入 | 输出 |
|---|---|---|---|
| **维护者烘焙** | 人(本地) | ECDICT `stardict.db` + Wiktionary + DeepSeek | `assets/{deck,enrichment,morphemes}.jsonl`(提交进仓库) |
| **用户首启** | `bootstrap()` 自动 | 内嵌的 `assets/*.jsonl` | `~/.tuna/tuna.db` |

用户路径调 `Deck::build_from_asset(assets::DECK)` + `load_enrichment_str(assets::ENRICHMENT)` +
`canonicalize_cognates()`,**不需要 ECDICT、不需要网络、不需要 LLM**。

## 维护者烘焙命令

```bash
tuna build-deck            # ECDICT(data/stardict.db)→ data/tuna.db
tuna export-deck           # data/tuna.db → assets/deck.jsonl(提交进仓库)
uv run scripts/bake.py     # Wiktionary 词源接地 → data/etym_cache.jsonl(gitignored)
uv run scripts/narrate.py  # 词根聚类 + 受控 LLM → assets/{morphemes,enrichment}.jsonl(提交)
```

> **注意**:`bake.py` / `narrate.py` 仍依赖 `uv` + Python。P7 砍掉的是**运行时** Python 依赖,
> 资产烘焙管线的 Python 依赖保留。用户安装 tuna 不需要 Python。

## Stage 1 — `bake.py`(确定性,零 LLM)

对每个词抓 Wiktionary etymology(pin `rev_id`)→ 确定性模板解析 → 分级。产物
`data/etym_cache.jsonl`(**gitignored**,存原始 etymology,parser 调整可秒级离线迭代,不重新抓网)。

### 分级类别

`cited-affix` / `cited-1hop` / `single-root` / `germanic` / `needs-review` / `no-page` / `no-etymology` / `no-templates`

### 关键约束

- **Wikimedia API 礼仪**:WORKERS=3 + retries + backoff + `maxlag` 参数。原 6 worker / 33 req/s 触发
  429 被当"无页面"误判 94.8% no-page。
- **解析器处理 `{{ety|:af}}` 包装 + prefer-latinate**:897 词 needs-review 多数可解析,因 Wiktionary
  新统一 ety 包装 + 拉丁词源未必是第一个(parent 经古法语);不靠 LLM 编造。
- **缓存合并 tie-break 优先"有 raw ety"**:原逻辑只比 category GOOD,导致加 raw 存储后全量重抓;
  修后断点续才正确。
- **single-root 词**:占 36%,没存 morpheme → 不入图。修复:用被引用的拉丁词源作为根节点锚入
  (如 `part → partem`),使其成为 partial/particle 的兄弟。decomposition 埋在拉丁 lemma 页深处难
  确定性触达,交给关笼 LLM 分解真实 Wiktionary 词源(grounded 且 flagged,不编造)。
- **日耳曼不可分解词**(`might`/`area`)老实给钩子,不编根。

## Stage 2 — `narrate.py`(关笼 LLM)

读 `etym_cache.jsonl` → 生成 `assets/morphemes.jsonl`(IDF 加权聚类)+ `assets/enrichment.jsonl`
(中文叙述 / 推导 / 例句)。

### 关笼 = LLM 拿到已核验词素作为不可变真值

LLM **只能**:
- 翻译词素成中文
- 串成推导链
- 写例句

LLM **加不进也改不了任何根**。所以它写出来的东西**编造词根在结构上不可能**——这正是要的诚实度。

### 置信级由流水线从证据算出,非 LLM 自盖章

`etymology_confidence`(`cited` / `folk` / `mnemonic`)由 `narrate.py` 从 Wiktionary 证据算出,每根可
回溯 `rev_id`。

### 并发与限流

- DeepSeek v4-flash 每次调用带内部推理约 15s,16 并发仅 ~1 词/秒 → 4521 词真实总耗时 1-2 小时。
- retry 必须 + backoff,WORKERS=32,实测 2.26 词/秒、0 报错(原 retry 无 backoff → 限流直接跳词)。

## 产物归属

| 文件 | 提交? | 内容 |
|---|---|---|
| `assets/deck.jsonl` | ✓ | 4801 考研词(ECDICT 范围内,2.2M) |
| `assets/enrichment.jsonl` | ✓ | DeepSeek 精加工(词素/推导/例句/图边) |
| `assets/morphemes.jsonl` | ✓ | 词素脊柱(几百条,人类可审) |
| `data/stardict.db` | ✗(gitignored) | 全量 ECDICT(851MB) |
| `data/tuna.db` | ✗(gitignored) | 开发库(`build-deck` 可再生) |
| `data/etym_cache.jsonl` | ✗(gitignored) | Wiktionary 原始词元(parser 迭代用) |

> `morphemes.jsonl` **不被运行时加载**——只 `enrichment.jsonl` 进库;morpheme id 来自
> `normalize_morpheme(unit)`。它存在是为人类可审的 spine。

## 运行时同源合并(用户首启时)

`bootstrap()` 在 `load_enrichment_str` 之后调 `Deck::canonicalize_cognates()`——一次确定性、
gloss-gated 的离线 pass,把 Wiktionary 对每个词给出的*直接*词源碎片折回最简词根:

- `inspect→spect`、`spectacle→spectāculum`、`spectator→spectate` → 同一 `spect` 节点
- **只在「一个折叠形是另一个的前缀」且「中文释义共享义符」时才合并**:`port`(拿/运)不会吞掉
  `portion`(部分),`spec`(种类)不会并入 `spect`(看)
- 词素本身保持 Wiktionary 原样,归并是可测试的透明变换(有单测
  `cognate_merge_reunites_roots_but_gloss_gate_holds`)
- `spect` 家族由此从 2 词并到 6 词;语法后缀降级为 `bond=0`,永不冒充"你学过的同根词"

这个 pass 在**用户机器上**跑(不是烘焙时),因为它只依赖已内嵌的 `enrichment.jsonl`,且对 assets
文件本身只读不改。

## 已知烘焙缺口(诚实注记)

- **needs-review 5%(~241 词)**:templates present 但 parser 漏 ≥2 词素;走 reconciliation agent / 人工审,
  不静默猜测。
- **`port` 家族英文 `port` 与 Latin `portō` 分裂**:已知聚类完整性缺口,honest not false。
- **dictate↔predict 跨类未连**:single-root(Latin `dictātum`)与 cited-affix(English `dict`)未结构化
  连接;留给 cited cognate 边作链接源补上,不做 hacky Latin-stemmer。
- **`ky` 标签 4801 词 vs 官方 ~5500 大纲**:ECDICT 的 `ky` 标签是词表来源,与官方大纲有差,列为
  backlog 精修项。
