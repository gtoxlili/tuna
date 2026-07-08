//! On-demand Socratic 辨析: the mirror in live action. When you ask about a word,
//! DeepSeek does NOT hand you a comparison table — it splits the roots, poses the
//! one question that lets you derive the difference yourself, and only then, in a
//! line or two, confirms. Amplify the reasoner; don't replace them.

use anyhow::Result;

use super::DeepSeek;

pub const SOCRATIC_SYSTEM: &str = "你是苏格拉底式的考研词汇导师。学习者正在学一个词，想弄清它与形近/近义词的区别。不要一上来就给结论或对照表。先把相关词各自的词根拆一行，然后抛出一个能让他自己推出区别的关键提问，留出思考的空间；最后只用一两句点出核心差异。语气克制，禁止鸡汤和花哨比喻，用中文，简短。";

/// Ask for a Socratic contrast of `word` given some context (its confusables + gloss).
pub fn socratic(client: &DeepSeek, model: &str, word: &str, context: &str) -> Result<String> {
    let user = format!(
        "目标词: {word}\n{context}\n请用苏格拉底式引导我分辨它和易混词，别直接把答案铺开。"
    );
    let (text, _usage) = client.chat_text(model, SOCRATIC_SYSTEM, &user, 900)?;
    Ok(text.trim().to_string())
}

pub const EVAL_SYSTEM: &str = "你是苏格拉底式词汇导师,专门点评学习者自己推出的词义。会给你:词、它已核验的真实词素、以及学习者的推测。点评他的**推理过程**,而不是对错评判:①先肯定他抓对的词素或方向;②如果有偏差,用一个提问指出是哪个词素/哪一步导致的,给他自己修正的空间;③最后给出正确的推导链。语气鼓励、克制,禁止鸡汤;用中文,简短(3-5 行)。这是一个把推词义当推理游戏的人,珍视他的思考。";

/// Evaluate the learner's OWN derivation guess against the grounded morphemes — a
/// live mirror that reflects his reasoning back, never a served verdict.
pub fn evaluate_guess(
    client: &DeepSeek,
    model: &str,
    word: &str,
    morphemes: &str,
    guess: &str,
) -> Result<String> {
    let ms = if morphemes.is_empty() {
        "（此词无清晰词素分解）".to_string()
    } else {
        morphemes.to_string()
    };
    let user = format!("词: {word}\n已核验词素: {ms}\n学习者的推测: 「{guess}」\n请点评他的推理。");
    let (text, _usage) = client.chat_text(model, EVAL_SYSTEM, &user, 700)?;
    Ok(text.trim().to_string())
}
