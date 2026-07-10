//! On-demand Socratic 辨析 + derivation chat: the mirror in live action. When you
//! ask about a word, DeepSeek does NOT hand you a comparison table — it splits the
//! roots, poses the one question that lets you derive the difference yourself, and
//! only then, in a line or two, confirms. Amplify the reasoner; don't replace them.

use anyhow::Result;

use super::DeepSeek;

pub const SOCRATIC_SYSTEM: &str = "你是苏格拉底式的考研词汇导师。学习者正在学一个词，想弄清它与形近/近义词的区别。不要一上来就给结论或对照表。先把相关词各自的词根拆一行，然后抛出一个能使他自己推出区别的关键提问，留出思考的空间；最后只用一两句点出核心差异。语气克制，禁止鸡汤和花哨比喻，用中文，简短。";

/// Ask for a Socratic contrast of `word` given some context (its confusables + gloss).
pub fn socratic(client: &DeepSeek, model: &str, word: &str, context: &str) -> Result<String> {
    let user = format!(
        "目标词: {word}\n{context}\n请用苏格拉底式引导我分辨它和易混词，别直接把答案铺开。"
    );
    let (text, _usage) = client.chat_text(model, SOCRATIC_SYSTEM, &user, 900)?;
    Ok(text.trim().to_string())
}

/// System prompt for the multi-turn derivation chat. The LLM knows the ground truth
/// (verified morphemes + meaning) but must guide the learner through questions, never
/// blurting the answer. Each reply is short to keep the conversation a dialogue.
pub const DERIVE_CHAT_SYSTEM: &str = "你是词根推导游戏的引导者。学习者正在尝试推导一个新词的意思，你会收到词、它已核验的真实词素和含义。规则：绝不直接说出正确词义；如果学习者方向正确，肯定他抓对的词素；如果有偏差，用一个提问引导他关注被忽略的词素；每次只回复 1-3 句，像对话一样自然；用中文。";

/// Continue a multi-turn derivation chat. Builds the full message history (system +
/// prior turns + new user message) and sends it to the LLM. `meaning` is the verified
/// gloss — the system prompt promises the model the ground-truth meaning (it must
/// guide toward it without blurting it); withholding it would leave the model
/// inventing its own "correct answer" and confidently steering the learner wrong.
pub fn derive_chat(
    client: &DeepSeek,
    model: &str,
    word: &str,
    morphemes: &str,
    meaning: &str,
    turns: &[(bool, String)], // (is_user, text)
    new_message: &str,
) -> Result<String> {
    let mut messages: Vec<(&str, String)> = vec![("system", DERIVE_CHAT_SYSTEM.to_string())];
    let morphemes_line = if morphemes.is_empty() {
        "（此词无清晰词素分解，引导学习者从词形联想与语境猜测）".to_string()
    } else {
        morphemes.to_string()
    };
    let info = format!("目标词: {word}\n已核验词素: {morphemes_line}\n真实含义（仅供引导，绝不直说）: {meaning}");
    messages.push(("user", info));
    messages.push((
        "assistant",
        "好，我来引导你推导这个词。说说你看到了哪些熟悉的词素？".to_string(),
    ));
    for (is_user, text) in turns {
        messages.push((if *is_user { "user" } else { "assistant" }, text.clone()));
    }
    messages.push(("user", new_message.to_string()));
    let text = client.chat_multi(model, messages, 500)?;
    Ok(text.trim().to_string())
}
