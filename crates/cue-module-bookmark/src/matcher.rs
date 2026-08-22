//! fuzzy 匹配与打分(§27、§28)。复制自 cue-module-app(Rule of Three
//! 第二次使用,§72 允许重复;第三个消费者(FileModule)落地时下沉 util crate。
//! 打分倾向:前缀命中 > 连续命中 > 词首命中 > 离散命中;键越短越好。
//! 手写实现而非引入 nucleo:catalog 规模(千级)× 键数(3)下,
//! 线性扫描远低于 §79 的 P95 < 15 ms 预算(§72 克制原则)。

/// 对单个键做子序列 fuzzy 打分。不匹配(非子序列)返回 None。
pub fn fuzzy_score(query: &str, key: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().collect();
    let k: Vec<char> = key.chars().collect();
    if q.len() > k.len() {
        return None;
    }
    let mut score = 0i32;
    let mut qi = 0;
    let mut prev_matched = false;
    for (i, &c) in k.iter().enumerate() {
        // 键允许携带原始大小写(驼峰边界需要它),比较时归一到
        // ASCII 小写;调用方保证 query 已小写。
        if qi < q.len() && c.to_ascii_lowercase() == q[qi] {
            score += 10;
            if prev_matched {
                score += 6; // 连续命中
            }
            if i == 0 {
                score += 8; // 前缀命中
            } else if word_boundary(&k, i) {
                score += 4; // 词首命中
            }
            prev_matched = true;
            qi += 1;
        } else {
            prev_matched = false;
        }
    }
    if qi < q.len() {
        return None;
    }
    // 键长惩罚(有界):长名字离散命中排在短名字精确命中之后
    score -= ((k.len() - q.len()) as i32).min(20);
    Some(score)
}

fn word_boundary(k: &[char], i: usize) -> bool {
    matches!(k[i - 1], ' ' | '-' | '_' | '.') || (k[i - 1].is_lowercase() && k[i].is_uppercase())
}

/// 一个 entry 的多个键(name_lower / pinyin_full / initials)取最佳。
pub fn best_score(query: &str, keys: &[&str]) -> Option<i32> {
    keys.iter().filter_map(|k| fuzzy_score(query, k)).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_required() {
        assert!(fuzzy_score("zz", "visual studio code").is_none());
        assert!(fuzzy_score("vs", "visual studio").is_some());
    }

    #[test]
    fn prefix_beats_scattered() {
        let prefix = fuzzy_score("code", "code runner").unwrap();
        let scattered = fuzzy_score("code", "c++ open debugger extras").unwrap();
        assert!(prefix > scattered);
    }

    #[test]
    fn initials_key_matches() {
        // "yjwj" 对首字母键是精确前缀,对全拼键是离散命中
        let initials = fuzzy_score("yjwj", "yjwj").unwrap();
        let full = fuzzy_score("yjwj", "yongjiewujian").unwrap();
        assert!(initials > full);
    }

    #[test]
    fn shorter_key_wins() {
        let short = fuzzy_score("no", "no").unwrap();
        let long = fuzzy_score("no", "notepad plus plus").unwrap();
        assert!(short > long);
    }

    #[test]
    fn camel_boundary_bonus() {
        // 原始大小写键:命中落在驼峰大写字母上触发词首加分(+4)。
        let camel = fuzzy_score("vc", "VisualStudioCode").unwrap();
        let flat = fuzzy_score("vc", "visualstudiocode").unwrap();
        assert_eq!(camel - flat, 4);
    }

    #[test]
    fn mixed_case_key_matches_lowercase_query() {
        assert!(fuzzy_score("vscode", "VisualStudioCode").is_some());
    }
}
