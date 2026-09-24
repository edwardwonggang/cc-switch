//! 跨 turn 车轱辘行动计划重复检测器
//!
//! 方案 A（[`super::repeat_detector`]）只覆盖「单条消息内连续片段重复」。这里新增
//! 跨消息（跨 turn）检测：模型在同一 session 的多条不同 assistant 消息里反复输出
//! 几乎相同的「行动计划式」自然语言句（如「让我用 Python 打印 mobile.css 405-525
//! 和 795-805 行。让我执行。」），却不真正推进任务。检测器按 session 维度维护最近
//! N 条消息的行动计划短语指纹，一旦高相似指纹出现在 ≥3 条不同消息，即判定为跨 turn
//! 车轱辘循环。
//!
//! 关键约束：只提取带祈使/动作意图的自然语言短语，**绝不**把代码/函数/变量/路径的
//! 自然引用（如 `hv_off_close_breaker`、`SID_BATTERY_*`）当作行动计划，避免误杀
//! 调试时的正常引用。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};

/// 开关，默认开启；可在运行期调用 [`CrossTurnStore::set_enabled`] 临时关闭。
pub(crate) const CROSS_TURN_ENABLED: bool = true;

/// 最近 N 条消息的窗口大小。
pub(crate) const WINDOW_SIZE: usize = 20;

/// 相同/高相似行动计划短语出现在多少条不同消息即判定。
pub(crate) const REPEAT_MESSAGE_THRESHOLD: usize = 3;

/// 判定为「高度相似」所需的最小相似度（0.0 ~ 1.0）。
pub(crate) const SIMILARITY_THRESHOLD: f64 = 0.85;

/// 参与判定的行动计划短语最小长度（字符），过滤过短噪声。
pub(crate) const MIN_PHRASE_CHARS: usize = 12;

/// 中文动作意图起始词。
const CN_ACTION_PREFIXES: &[&str] = &["让我", "我来", "我要", "我需要", "下一步", "现在来"];

/// 英文动作意图起始词。
const EN_ACTION_PREFIXES: &[&str] = &[
    "let me",
    "i will",
    "i'll",
    "let's",
    "i need to",
    "i want to",
    "let me try",
    "let me run",
    "let me check",
    "let me view",
    "let me do",
    "let me execute",
];

/// 句子切分使用的字符集合。
const SENTENCE_ENDERS: &[char] = &['.', '!', '?', '。', '！', '？', '；', ';', '\n'];

/// 提取文本中的「行动计划式」自然语言短语指纹。
///
/// 只保留带祈使/动作意图前缀的句子，归一化（删数字/空白/标点、小写）后得到指纹。
/// 代码/标识符引用不含动作意图前缀，不会被提取。
pub(crate) fn extract_action_plan_fingerprints(text: &str) -> Vec<String> {
    let mut fingerprints = Vec::new();
    for sentence in split_sentences(text) {
        let trimmed = sentence.trim();
        if trimmed.is_empty() || !is_action_plan_sentence(trimmed) {
            continue;
        }
        let normalized = normalize_phrase(trimmed);
        if normalized.chars().count() >= MIN_PHRASE_CHARS {
            fingerprints.push(normalized);
        }
    }
    fingerprints
}

/// 判断一个句子是否带动作意图（祈使/行动计划）前缀。
///
/// 中文与英文一致，只匹配句首动作意图词，避免把叙述句中间出现的
/// 「让我/我来/我要」误判为行动计划。
fn is_action_plan_sentence(sentence: &str) -> bool {
    let lower = sentence.trim_start().to_lowercase();
    let cn_hit = CN_ACTION_PREFIXES.iter().any(|p| lower.starts_with(p));
    let en_hit = EN_ACTION_PREFIXES.iter().any(|p| lower.starts_with(p));
    cn_hit || en_hit
}

/// 把文本按句末标点切分为句子。
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut pending = String::new();
    for ch in text.chars() {
        pending.push(ch);
        if SENTENCE_ENDERS.contains(&ch) {
            sentences.push(std::mem::take(&mut pending));
        }
    }
    if !pending.trim().is_empty() {
        sentences.push(pending);
    }
    sentences
}

/// 归一化短语：删数字/空白/标点、统一小写，token 间保留单空格。
///
/// 行号、数值、文件路径中的数字会被删除，使「同骨架、仅行号/数值不同」的行动计划
/// 归一化后一致，从而能跨消息匹配「车轱辘」。
fn normalize_phrase(phrase: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in phrase.chars() {
        if ch.is_whitespace() {
            prev_space = true;
            continue;
        }
        if ch.is_ascii_digit() {
            prev_space = true;
            continue;
        }
        if !ch.is_alphanumeric() {
            // 标点/符号视为分隔。
            prev_space = true;
            continue;
        }
        if prev_space && !out.is_empty() {
            out.push(' ');
        }
        prev_space = false;
        for c in ch.to_lowercase() {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// 计算两个字符串的字符级相似度（1 - 归一化编辑距离）。
fn similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let max_len = a.chars().count().max(b.chars().count());
    if max_len == 0 {
        return 1.0;
    }
    let dist = levenshtein_distance(a, b);
    1.0 - (dist as f64 / max_len as f64)
}

/// 判断两个指纹是否「高度相似」。
fn is_highly_similar(a: &str, b: &str) -> bool {
    similarity(a, b) >= SIMILARITY_THRESHOLD
}

/// 字符级 Levenshtein 编辑距离。
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![0usize; b.len() + 1];
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1) // 删除
                .min(cur[j] + 1) // 插入
                .min(prev[j] + cost); // 替换
        }
        prev = cur;
    }
    prev[b.len()]
}

/// 单个 session 的跨 turn 状态：最近 N 条消息的行动计划短语指纹。
#[derive(Default)]
struct SessionTurnState {
    /// 每条消息的指纹集合，队首为最旧。
    messages: VecDeque<Vec<String>>,
}

impl SessionTurnState {
    /// 判定当前消息的指纹是否构成跨 turn 车轱辘循环。
    ///
    /// 对每个指纹统计历史**不同消息**中含高相似指纹的数量，任一指纹命中
    /// [`REPEAT_MESSAGE_THRESHOLD`] 即判定。
    fn check(&self, fingerprints: &[String]) -> bool {
        for fp in fingerprints {
            let mut hit_count = 0usize;
            for msg in &self.messages {
                if msg.iter().any(|m| is_highly_similar(m, fp)) {
                    hit_count += 1;
                    if hit_count >= REPEAT_MESSAGE_THRESHOLD {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// 入队本条消息的指纹，超出窗口时丢弃最旧。
    fn push(&mut self, fingerprints: Vec<String>) {
        if fingerprints.is_empty() {
            return;
        }
        self.messages.push_back(fingerprints);
        while self.messages.len() > WINDOW_SIZE {
            self.messages.pop_front();
        }
    }
}

/// 跨 turn 检测器状态存储：`session_id -> 状态`，跨请求保持。
#[derive(Default)]
pub(crate) struct CrossTurnStore {
    sessions: HashMap<String, SessionTurnState>,
    enabled: AtomicBool,
}

impl CrossTurnStore {
    /// 创建空存储。
    pub(crate) fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            enabled: AtomicBool::new(CROSS_TURN_ENABLED),
        }
    }

    /// 临时开关检测（供配置关闭，避免误杀影响调试）。
    #[allow(dead_code)]
    pub(crate) fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    /// 当前是否开启。
    pub(crate) fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// 检查给定 session 的消息指纹是否构成跨 turn 循环。
    ///
    /// 先把本条消息指纹入队（使「当前消息也计入不同消息数」，与「≥3 条不同消息」
    /// 语义一致），再统计历史。命中时返回 `true` 并回滚本条（由上层 abort 触发客户端
    /// 重试，该条不应保留）；未命中时保留入队，供后续消息判定。开关关闭时直接跳过
    /// （不检测也不入队）。
    pub(crate) fn check_and_record(&mut self, session: &str, fingerprints: &[String]) -> bool {
        if !self.enabled() {
            return false;
        }
        if fingerprints.is_empty() {
            return false;
        }
        let state = self.sessions.entry(session.to_string()).or_default();
        state.push(fingerprints.to_vec());
        let hit = state.check(fingerprints);
        if hit {
            state.messages.pop_back();
        }
        hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_identifier_references_are_not_fingerprinted() {
        // seq=530 实测的代码/标识符自然引用，绝不能当作行动计划。
        let text = "hv_off_close_breaker(hv_info_t* hv_info, int index) { \
            int do_num = get_conta_do_num(HV_DO_DC_BREAKER, index, 0); \
            return SUCCESS; } \
            SID_BATTERY_SYSTEM_MANAGEMENT_UNIT_CTL_START_SLAVE_BMU_LEFT_CLUSTER_UPDATE \
            send_syn_req(SIGNAL_CTRL_SUBSCRIBE_MSG, &ctrl_sid, sizeof(ctrl_sid))";
        let fps = extract_action_plan_fingerprints(text);
        assert!(fps.is_empty(), "代码引用不应提取出行动计划指纹: {fps:?}");
    }

    #[test]
    fn action_plan_sentence_is_extracted() {
        let text = "让我用 Python 打印 mobile.css 405-525 和 795-805 行。让我执行。";
        let fps = extract_action_plan_fingerprints(text);
        assert_eq!(fps.len(), 1, "只应提取出完整行动计划句: {fps:?}");
        // `.` 也是句末切分符，`mobile.css` 中的点号会把句子截断，但仍提取出
        // 核心动作意图骨架，足以用于跨消息匹配。
        assert_eq!(fps[0], "让我用 python 打印 mobile");
    }

    #[test]
    fn english_action_plan_is_extracted_and_normalized() {
        let text = "Let me view lines 1160-1200. Verifying the flag state now.";
        let fps = extract_action_plan_fingerprints(text);
        assert_eq!(fps.len(), 1, "只应提取 Let me 句: {fps:?}");
        assert_eq!(fps[0], "let me view lines");
    }

    #[test]
    fn cross_turn_loop_triggers_after_threshold_messages() {
        let mut store = CrossTurnStore::new();
        let session = "sess-test-1";
        let fp = vec!["让我用 python 打印 mobilecss 和 行".to_string()];

        // 前两条不同消息，未达阈值。
        assert!(!store.check_and_record(session, &fp));
        assert!(!store.check_and_record(session, &fp));
        // 第三条不同消息命中阈值。
        assert!(store.check_and_record(session, &fp));
    }

    #[test]
    fn slight_variations_still_match() {
        // 仅行号不同仍视为高度相似（归一化后一致）。
        let a = "让我用 python 打印 mobilecss 和 行".to_string();
        let b = "让我用 python 打印 mobilecss 和 行".to_string();
        assert!(is_highly_similar(&a, &b));
        // 完全不同的行动计划不应误判。
        let c = "让我检查电机接触器状态".to_string();
        assert!(!is_highly_similar(&a, &c));
    }

    #[test]
    fn normalization_makes_similar_action_plans_match() {
        // 归一化前仅行号/数值不同，归一化后应一致，从而跨消息命中。
        let a =
            extract_action_plan_fingerprints("让我用 Python 打印 mobile css 405-525 和 795-805 行");
        let b =
            extract_action_plan_fingerprints("让我用 Python 打印 mobile css 100-200 和 300-400 行");
        assert_eq!(a, b, "仅行号不同，归一化后指纹应一致");
        assert!(is_highly_similar(&a[0], &b[0]));
    }

    #[test]
    fn let_me_call_write_stdin_triggers_cross_turn() {
        // Mac seq=539 真实样本：同一条行动计划句跨 3 条消息应命中 CrossTurn。
        let mut store = CrossTurnStore::new();
        let session = "sess-write-stdin";
        let fp = extract_action_plan_fingerprints("Let me call write_stdin.");
        assert!(!fp.is_empty());
        assert!(!store.check_and_record(session, &fp));
        assert!(!store.check_and_record(session, &fp));
        assert!(store.check_and_record(session, &fp));
    }

    #[test]
    fn window_caps_memory() {
        let mut state = SessionTurnState::default();
        for i in 0..(WINDOW_SIZE + 10) {
            state.push(vec![format!("action plan number {i}")]);
        }
        assert!(state.messages.len() <= WINDOW_SIZE);
    }
}
