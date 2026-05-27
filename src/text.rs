//! import 共通のテキスト sanitize。
//!
//! 外部サービス（GCal / GitHub）由来の文字列には ANSI escape (`\x1b[...`)、NUL、
//! Unicode の BIDI override / 不可視文字 (U+202E, U+200E など) が含まれうる。
//! TUI / ログでの偽装表示や ybasey 側で文字列フィールドが壊れるのを避けるため、
//! import 時点で危険な文字を除去する。
//!
//! 除去対象:
//! - `c.is_control()`: C0/C1 制御文字 (U+0000-U+001F, U+007F-U+009F)
//! - Cf (Format): ZWJ, BIDI mark, BIDI override, LANGUAGE TAG など
//! - Zl (Line Separator) U+2028, Zp (Paragraph Separator) U+2029
//!
//! 例外として `\t` (HT, U+0009) のみ残す (CLI 出力時に有用な場合があるため)。

pub fn sanitize_text(s: &str) -> String {
    s.chars().filter(|c| keep_char(*c)).collect()
}

fn keep_char(c: char) -> bool {
    if c == '\t' {
        return true;
    }
    if c.is_control() {
        return false;
    }
    // Unicode format / line separator / paragraph separator を除去
    matches!(c as u32,
        0..=0x1F | 0x7F..=0x9F |
        0x200B..=0x200F | 0x202A..=0x202E |
        0x2060..=0x206F |
        0x2028 | 0x2029 |
        0xFFF9..=0xFFFB |
        0xE0000..=0xE007F
    ).then(|| false).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_normal_text() {
        assert_eq!(sanitize_text("バグ修正 #12"), "バグ修正 #12");
    }

    #[test]
    fn removes_control_chars() {
        // ESC (制御文字) と NUL を除去。後続の通常文字は残す
        assert_eq!(sanitize_text("a\x1bb\x00c"), "abc");
    }

    #[test]
    fn removes_bidi_override() {
        // U+202E (RIGHT-TO-LEFT OVERRIDE) を除去
        assert_eq!(sanitize_text("a\u{202E}b"), "ab");
    }

    #[test]
    fn keeps_tab() {
        assert_eq!(sanitize_text("a\tb"), "a\tb");
    }
}
