//! 拼音索引(§27):全拼 + 首字母。load 期预计算,查询期零 IO。

use pinyin::ToPinyin;

/// 返回 (全拼键, 首字母键)。ASCII 字符不进拼音键——
/// 英文名已由 `name_lower` 键覆盖(§27 示例中 "naraka" 这类
/// 英文名/alias 的价值由 name_lower 承担)。
pub fn keys(name: &str) -> (String, String) {
    let mut full = String::new();
    let mut initials = String::new();
    for ch in name.chars() {
        if ch.is_ascii() {
            continue;
        }
        if let Some(py) = ch.to_pinyin() {
            let plain = py.plain();
            full.push_str(plain);
            if let Some(first) = plain.chars().next() {
                initials.push(first);
            }
        }
    }
    (full, initials)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinyin_keys() {
        let (full, initials) = keys("永劫无间");
        assert_eq!(full, "yongjiewujian");
        assert_eq!(initials, "yjwj");
    }

    #[test]
    fn mixed_name() {
        let (full, initials) = keys("微信 (WeChat)");
        assert_eq!(full, "weixin");
        assert_eq!(initials, "wx");
    }
}
