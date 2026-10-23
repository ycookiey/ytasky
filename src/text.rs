//! Text sanitizing shared by the imports.
//!
//! Strings from external services (Google Calendar, GitHub) may contain ANSI escapes
//! (`\x1b[...`), NUL, and Unicode BIDI overrides or invisible characters (U+202E,
//! U+200E and so on). They are removed at import time so that they cannot spoof
//! the TUI or log output or break string fields in the database.
//!
//! Removed:
//! - `c.is_control()`: C0/C1 control characters (U+0000-U+001F, U+007F-U+009F)
//! - Cf (Format): ZWJ, BIDI marks, BIDI overrides, LANGUAGE TAG and so on
//! - Zl (Line Separator) U+2028, Zp (Paragraph Separator) U+2029
//!
//! Only `\t` (HT, U+0009) is kept, because it can be useful in CLI output.

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
    // Remove Unicode format, line separator and paragraph separator characters
    !matches!(c as u32,
        0x180E |                        // Mongolian Vowel Separator (Cf)
        0x200B..=0x200F |               // ZWSP, ZWNJ, ZWJ, LRM, RLM
        0x202A..=0x202E |               // BIDI override
        0x2028 | 0x2029 |               // Line/Paragraph Separator
        0x2060..=0x206F |               // WJ, invisible operators, deprecated Cf
        0xFE00..=0xFE0F |               // Variation Selectors 1-16
        0xFEFF |                        // BOM / ZWNBSP
        0xFFF9..=0xFFFB |               // Interlinear annotation
        0xE0000..=0xE007F |             // Language tags
        0xE0100..=0xE01EF               // Variation Selectors 17-256
    )
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
        // Remove ESC (a control character) and NUL; the following characters stay
        assert_eq!(sanitize_text("a\x1bb\x00c"), "abc");
    }

    #[test]
    fn removes_bidi_override() {
        // Remove U+202E (RIGHT-TO-LEFT OVERRIDE)
        assert_eq!(sanitize_text("a\u{202E}b"), "ab");
    }

    #[test]
    fn keeps_tab() {
        assert_eq!(sanitize_text("a\tb"), "a\tb");
    }

    #[test]
    fn removes_bom_and_variation_selectors() {
        // U+FEFF (BOM)
        assert_eq!(sanitize_text("\u{FEFF}Hello"), "Hello");
        // U+180E (Mongolian Vowel Separator)
        assert_eq!(sanitize_text("a\u{180E}b"), "ab");
        // U+FE0F (Variation Selector-16)
        assert_eq!(sanitize_text("a\u{FE0F}b"), "ab");
        // U+E0100 (Variation Selector-17)
        assert_eq!(sanitize_text("a\u{E0100}b"), "ab");
    }
}
