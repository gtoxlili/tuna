//! The two AI chat modes + the CLI one-shot. Both chat modes hand the model the
//! verified facts (word, morphemes, real meaning, known confusables) and leave the
//! conversation to it — the harness supplies ground truth, not a script. The only
//! hard rules are the ones that never vary: derive mode never states the meaning
//! (the learner's job is to derive it), replies stay short and in Chinese.

use anyhow::Result;

use super::DeepSeek;

/// One-shot Socratic contrast, used by the CLI `tuna ask <word>` command (prints to
/// stdout, no conversation to hold).
pub const SOCRATIC_SYSTEM: &str = "你是考研词汇导师。学习者想弄清一个词与形近/近义词的区别。先把相关词各自的词根拆一行，然后提出一个能让他自己推出区别的关键提问，最后用一两句点出核心差异。语气克制，中文，简短。";

/// Ask for a Socratic contrast of `word` given some context (its confusables + gloss).
pub fn socratic(client: &DeepSeek, model: &str, word: &str, context: &str) -> Result<String> {
    let user = format!("目标词: {word}\n{context}\n请引导我分辨它和易混词。");
    let (text, _usage) = client.chat_text(model, SOCRATIC_SYSTEM, &user, 8192)?;
    Ok(text.trim().to_string())
}

/// System prompt for the derive chat (new word, pre-reveal). The model holds the
/// ground truth so it can steer accurately; the game's one invariant is that the
/// learner derives the MEANING himself — everything else (grammar, usage) is
/// scaffolding knowledge and gets answered plainly.
pub const DERIVE_CHAT_SYSTEM: &str = "你是词根推导环节的引导者，帮考研学习者从词素推出一个新词的意思。你会收到：目标词、已核验的词素、真实词义（学习者此刻看不到，仅供你校准方向）。红线只有一条且只管词义：不把词义直接告诉他，他要自己推出来。他说对的部分予以确认；有偏差时，用一个针对具体词素的提问把他引回来。词义之外的问题（语法、用法、为什么这么说）不属于谜题，直接用大白话讲清楚。学习者几乎没有语法基础，确需术语时当场用几个字解释。每次回复 1-3 句，中文，像对话。";

/// System prompt for the compare chat (post-reveal / review). The learner has seen
/// the meaning; the goal is telling the word apart from its neighbours.
pub const COMPARE_CHAT_SYSTEM: &str = "你是考研词汇辨析导师，帮学习者分清一个词与它的形近/近义词。你会收到：目标词、词义、已标注的易混/近义词（可能为空，为空时自行挑最值得对比的词）。开场先把相关词各自的词根拆一行，再提一个能让他自己推出区别的问题；他回应后确认或纠偏，用一两句点出核心差异。他追问时直接回答，先给结论再展开。学习者几乎没有语法基础，确需术语时当场用几个字解释。每次回复简短，中文。";

/// System prompt for the grammar chat (an example sentence, post-reveal). Grammar
/// is support knowledge, not the derivation game: conclusions come FIRST, plainly —
/// the learner starts from zero grammar, asks "why can't I say X", and deserves
/// the answer.
pub const GRAMMAR_CHAT_SYSTEM: &str = "你是英语句子讲解者，服务几乎零语法基础的考研学习者。你会收到一个例句（含中文翻译）和它出自的单词。用大白话讲，确需术语时当场用几个字解释（如“介词，挂名词用的小词”）。开场：先一句话说这个句子的骨架（谁、做什么、对什么），再说目标词在句中的角色和为什么需要它。之后他问什么就直接答什么，先给结论，再用一个极简的对照例子说明。每次回复不超过 5 句，中文。";

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
    let info = format!(
        "目标词: {word}\n已核验词素: {morphemes_line}\n真实含义（仅供引导，绝不直说）: {meaning}"
    );
    messages.push(("user", info));
    messages.push((
        "assistant",
        "好，我来引导你推导这个词。说说你看到了哪些熟悉的词素？".to_string(),
    ));
    for (is_user, text) in turns {
        messages.push((if *is_user { "user" } else { "assistant" }, text.clone()));
    }
    messages.push(("user", new_message.to_string()));
    // Deliberately roomy: reasoning models spend their chain-of-thought from the
    // same max_tokens budget, and a tight cap gets fully consumed by the thinking
    // — the answer comes back empty. The replies themselves stay short (the
    // system prompt asks for it); the budget is for the chain.
    let text = client.chat_multi(model, messages, 8192)?;
    Ok(text.trim().to_string())
}

/// Continue a multi-turn compare chat. An empty `new_message` is the kickoff: the
/// facts message stands alone and the model opens with the contrast lead-in, so
/// pressing `a` delivers the distinction without composing an opening question.
pub fn compare_chat(
    client: &DeepSeek,
    model: &str,
    word: &str,
    meaning: &str,
    neighbours: &str,
    turns: &[(bool, String)], // (is_user, text)
    new_message: &str,
) -> Result<String> {
    let mut messages: Vec<(&str, String)> = vec![("system", COMPARE_CHAT_SYSTEM.to_string())];
    let nb = if neighbours.is_empty() {
        "（无已标注的易混词）".to_string()
    } else {
        neighbours.to_string()
    };
    let info = format!("目标词: {word}\n词义: {meaning}\n易混/近义: {nb}");
    messages.push(("user", info));
    for (is_user, text) in turns {
        messages.push((if *is_user { "user" } else { "assistant" }, text.clone()));
    }
    if !new_message.is_empty() {
        messages.push(("user", new_message.to_string()));
    }
    // Deliberately roomy: reasoning models spend their chain-of-thought from the
    // same max_tokens budget, and a tight cap gets fully consumed by the thinking
    // — the answer comes back empty. The replies themselves stay short (the
    // system prompt asks for it); the budget is for the chain.
    let text = client.chat_multi(model, messages, 8192)?;
    Ok(text.trim().to_string())
}

/// Continue a multi-turn grammar chat about one example sentence. An empty
/// `new_message` is the kickoff: the model opens with the sentence's skeleton and
/// the target word's role in it.
pub fn grammar_chat(
    client: &DeepSeek,
    model: &str,
    word: &str,
    sentence_en: &str,
    sentence_zh: &str,
    turns: &[(bool, String)], // (is_user, text)
    new_message: &str,
) -> Result<String> {
    let mut messages: Vec<(&str, String)> = vec![("system", GRAMMAR_CHAT_SYSTEM.to_string())];
    let zh = if sentence_zh.is_empty() {
        String::new()
    } else {
        format!("\n中文翻译: {sentence_zh}")
    };
    let info = format!("目标词: {word}\n例句: {sentence_en}{zh}");
    messages.push(("user", info));
    for (is_user, text) in turns {
        messages.push((if *is_user { "user" } else { "assistant" }, text.clone()));
    }
    if !new_message.is_empty() {
        messages.push(("user", new_message.to_string()));
    }
    // Deliberately roomy: reasoning models spend their chain-of-thought from the
    // same max_tokens budget; a tight cap comes back as an empty answer.
    let text = client.chat_multi(model, messages, 8192)?;
    Ok(text.trim().to_string())
}
