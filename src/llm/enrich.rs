//! Per-word enrichment: the DeepSeek touchpoint that turns a word into raw material
//! for derivation. It is a mirror, not a crutch — it hands over morphemes, the
//! anchors you likely already own, and a derive-it-yourself puzzle; it never asks
//! you to passively read a paragraph. Etymology is flagged honestly (solid/folk/
//! mnemonic) so a fabricated root is never dressed up as real.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{DeepSeek, Usage};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Enrichment {
    pub word: String,
    #[serde(default)]
    pub pos: String,
    #[serde(default)]
    pub ipa: String,
    #[serde(default)]
    pub gloss_zh: String,
    #[serde(default)]
    pub freq_tier: String,
    #[serde(default)]
    pub decomposable: bool,
    #[serde(default)]
    pub morphemes: Vec<Morpheme>,
    #[serde(default)]
    pub derivation_zh: String,
    #[serde(default)]
    pub etymology_confidence: String,
    #[serde(default)]
    pub known_anchors: Vec<String>,
    #[serde(default)]
    pub hook: String,
    #[serde(default)]
    pub graph_edges: Vec<GraphEdge>,
    #[serde(default)]
    pub collocations: Vec<String>,
    #[serde(default)]
    pub examples: Vec<Example>,
    #[serde(default)]
    pub derive_puzzle: Option<DerivePuzzle>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Morpheme {
    pub unit: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub meaning_zh: String,
    #[serde(default)]
    pub gloss_en: String,
    #[serde(default)]
    pub cognates: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GraphEdge {
    pub target: String,
    #[serde(default)]
    pub relation: String,
    #[serde(default)]
    pub via: String,
    #[serde(default)]
    pub why_zh: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Example {
    #[serde(default)]
    pub en: String,
    #[serde(default)]
    pub zh: String,
    #[serde(default)]
    pub level: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DerivePuzzle {
    #[serde(default)]
    pub given_zh: String,
    #[serde(default)]
    pub ask_zh: String,
    #[serde(default)]
    pub answer_zh: String,
}

/// Byte-stable system prefix — keep it constant so DeepSeek's prompt cache applies.
pub const SYSTEM_PROMPT: &str = "你是考研英语词汇的词源拆解引擎。对给定的词输出一个严格符合 schema 的 json 对象。硬规则：①known_anchors 只用学习者可能已掌握的 CET-4 基础词；②词源必须诚实——真实词源标 etymology_confidence=solid，教学有用但非严格的俗词源标 folk，纯记忆钩子标 mnemonic，禁止编造词根；③derivation_zh 写成一条推导链『A + B → … → 词义』，像推公式，不要写成解释段落；④examples 两句，第一句用 CET-4 词汇改写，第二句贴近考研真题的学术书面风格并标 level=考研；⑤decomposable=false 时 morphemes 可为空，用 hook 兜底。schema = {\"word\":str,\"pos\":str,\"ipa\":str,\"gloss_zh\":str,\"freq_tier\":\"高频|中频|低频\",\"decomposable\":bool,\"morphemes\":[{\"unit\":str,\"type\":\"prefix|root|suffix\",\"meaning_zh\":str,\"gloss_en\":str,\"cognates\":[str]}],\"derivation_zh\":str,\"etymology_confidence\":\"solid|folk|mnemonic\",\"known_anchors\":[str],\"hook\":str,\"graph_edges\":[{\"target\":str,\"relation\":\"cognate_root|synonym|antonym|confusable\",\"via\":str,\"why_zh\":str}],\"collocations\":[str],\"examples\":[{\"en\":str,\"zh\":str,\"level\":str}],\"derive_puzzle\":{\"given_zh\":str,\"ask_zh\":str,\"answer_zh\":str}}";

/// Enrich one word. `known` are words the learner already owns (passed so the model
/// prefers anchors they've actually seen). Returns the parsed enrichment + raw JSON
/// (stored verbatim) + token usage.
pub fn enrich_word(
    client: &DeepSeek,
    model: &str,
    word: &str,
    known: &[String],
) -> Result<(Enrichment, String, Usage)> {
    let known_list = if known.is_empty() {
        "（暂无，请从 CET-4 词中挑选合适锚点）".to_string()
    } else {
        known.join(", ")
    };
    let user = format!("word: {word}\n该学习者已掌握的同根/基础词: [{known_list}]\n请只输出 json。");
    // Polysemous words (state, government) produce long objects; give ample room
    // so the JSON never truncates mid-object.
    let (content, usage) = client.chat_json(model, SYSTEM_PROMPT, &user, 3200)?;
    let enrichment: Enrichment = serde_json::from_str(&content)
        .with_context(|| format!("parsing enrichment JSON for '{word}': {content}"))?;
    Ok((enrichment, content, usage))
}
