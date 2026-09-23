//! 模型输出重复循环检测器
//!
//! 模型在长上下文解码退化时会反复输出几乎相同的文本（如同一句工具调用意图
//! 连续出现数百次），直到吃满输出上限。该模块提供纯函数式的检测逻辑，供
//! 流式响应的前置短缓冲阶段使用：一旦判定「同一短片段连续重复」，就中止上游
//! 并让客户端（Codex）自动重试，避免把 3 万字符的垃圾重复文本回灌给用户。

/// 判定为循环所需的最小连续重复次数。
///
/// 正常输出几乎不可能连续多个短片段完全相同，取 8 作为保守阈值可大幅压低
/// 误判率；deepseek-v4-flash 退化案例中同一句重复 376 次，远高于此值。
pub(crate) const REPEAT_THRESHOLD: usize = 8;

/// 参与重复判定片段的最小字符数，避免把过短的碎片（如单个标点、空格）计入。
pub(crate) const MIN_SEGMENT_CHARS: usize = 10;

/// 一次片段累积后，重新开始计数前允许的字符数（用于限制检测器占用内存）。
pub(crate) const MAX_PENDING_CHARS: usize = 4096;

/// 句子切分使用的字符集合：遇到其中任意字符即认为一个片段结束。
const SENTENCE_ENDERS: &[char] = &['.', '!', '?', '。', '！', '？', '\n'];

/// 文本重复检测器。
///
/// 使用方式：把上游流式增量文本持续交给 [`RepeatDetector::push`]，内部会按句末
/// 标点切出短片段，并统计「连续相同片段」的次数。当连续相同次数达到
/// [`REPEAT_THRESHOLD`] 时返回 `true`，表示检测到重复生成循环。
#[derive(Debug, Default)]
pub(crate) struct RepeatDetector {
    /// 尚未切分成完整片段的累积文本。
    pending: String,
    /// 最近一个参与判定的短片段（用于比较是否连续重复）。
    last_segment: Option<String>,
    /// 当前「与 last_segment 相同的连续片段」个数。
    consecutive_count: usize,
}

impl RepeatDetector {
    /// 创建空检测器。
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 追加一段流式增量文本，返回是否检测到重复循环。
    ///
    /// 内部把累积文本按句末标点切分为短片段，切出的片段若长度 >=
    /// [`MIN_SEGMENT_CHARS`] 则参与连续重复计数。未切完的残留文本保留到下次。
    pub(crate) fn push(&mut self, delta: &str) -> bool {
        if self.pending.len() + delta.len() > MAX_PENDING_CHARS {
            // 防御：正常长输出也应能正常放行，丢弃过长的未切分残留。
            self.pending.clear();
        }
        self.pending.push_str(delta);

        while let Some(segment) = Self::take_next_segment(&mut self.pending) {
            if segment.chars().count() < MIN_SEGMENT_CHARS {
                continue;
            }

            let trimmed = segment.trim();
            if trimmed.is_empty() {
                continue;
            }

            let is_same = self
                .last_segment
                .as_deref()
                .is_some_and(|last| last == trimmed);
            if is_same {
                self.consecutive_count += 1;
            } else {
                self.last_segment = Some(trimmed.to_string());
                self.consecutive_count = 1;
            }

            if self.consecutive_count >= REPEAT_THRESHOLD {
                return true;
            }
        }

        false
    }

    /// 从累积缓冲中切出下一个「以句末标点结尾的短片段」，返回 `None` 表示不足。
    fn take_next_segment(pending: &mut String) -> Option<String> {
        let mut first_end: Option<usize> = None;
        for (idx, ch) in pending.char_indices() {
            if SENTENCE_ENDERS.contains(&ch) {
                first_end = Some(idx);
                break;
            }
        }

        let end = first_end?;
        // 片段 = 从缓冲开头到第一个句末标点（含标点）。
        let split_at =
            end + pending[end..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        let segment = pending[..split_at].to_string();
        pending.drain(..split_at);
        Some(segment)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_diverse_text_never_triggers() {
        let mut detector = RepeatDetector::new();
        let text = "First line of output. Second distinct sentence. Third is different. \
Fourth continues. Fifth wraps up. Sixth here. Seventh moves on. Eighth more. Ninth distinct. \
Tenth final.";
        assert!(!detector.push(text));
    }

    #[test]
    fn repeated_sentence_triggers_after_threshold() {
        let mut detector = RepeatDetector::new();
        // 前几句正常，随后同一句连续重复超过阈值。
        let normal = "Let me check the function. Verifying the flag. ";
        assert!(!detector.push(normal));
        for _ in 0..REPEAT_THRESHOLD - 1 {
            assert!(!detector.push("Let me view lines 1160-1200. "));
        }
        // 连续第 REPEAT_THRESHOLD 次重复应触发。
        assert!(detector.push("Let me view lines 1160-1200. "));
    }

    #[test]
    fn interrupted_repetition_resets_count() {
        let mut detector = RepeatDetector::new();
        for _ in 0..REPEAT_THRESHOLD - 1 {
            assert!(!detector.push("Same repeated text here. "));
        }
        // 中间插入不同内容后，连续计数被重置。
        assert!(!detector.push("A different sentence in between. "));
        for _ in 0..REPEAT_THRESHOLD - 1 {
            assert!(!detector.push("Same repeated text here. "));
        }
        assert!(detector.push("Same repeated text here. "));
    }

    #[test]
    fn short_fragments_are_ignored() {
        let mut detector = RepeatDetector::new();
        // 片段过短（少于 MIN_SEGMENT_CHARS）不计入重复。
        for _ in 0..REPEAT_THRESHOLD + 2 {
            assert!(!detector.push("hi. "));
        }
        // 稍长但完整的一句话，重复超过阈值应触发。
        let sentence = "This is a long enough repeated sentence to count. ";
        for _ in 0..REPEAT_THRESHOLD - 1 {
            assert!(!detector.push(sentence));
        }
        assert!(detector.push(sentence));
    }
}
