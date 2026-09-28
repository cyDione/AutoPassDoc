//! Character-level helpers shared by structure detection, metadata and tokenizing.

/// Maps full-width ASCII forms and the ideographic space to their half-width
/// equivalents. One char maps to one char, so offsets stay aligned.
pub(crate) fn normalize_char(c: char) -> char {
    match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{3000}' => ' ',
        _ => c,
    }
}

pub(crate) fn normalize(s: &str) -> String {
    s.chars().map(normalize_char).collect()
}

pub(crate) fn is_han(c: char) -> bool {
    matches!(c,
        '\u{3007}'
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{20000}'..='\u{2FA1F}')
}

fn cn_digit(c: char) -> Option<u32> {
    Some(match c {
        '零' | '〇' | '○' | 'O' | 'o' | 'Ο' | '0' => 0,
        '一' | '1' => 1,
        '二' | '两' | '2' => 2,
        '三' | '3' => 3,
        '四' | '4' => 4,
        '五' | '5' => 5,
        '六' | '6' => 6,
        '七' | '7' => 7,
        '八' | '8' => 8,
        '九' | '9' => 9,
        _ => return None,
    })
}

/// Parses Arabic or Chinese numerals: `12`, `二十一`, `一百零五`, `二〇二四`.
/// Numerals without unit characters are read digit by digit.
pub(crate) fn parse_number(s: &str) -> Option<u32> {
    if s.is_empty() {
        return None;
    }
    if !s.chars().any(|c| matches!(c, '十' | '百' | '千' | '万')) {
        return s.chars().try_fold(0u32, |acc, c| {
            acc.checked_mul(10)?.checked_add(cn_digit(c)?)
        });
    }
    let (mut total, mut section, mut digit) = (0u32, 0u32, None::<u32>);
    for c in s.chars() {
        let unit = match c {
            '十' => 10,
            '百' => 100,
            '千' => 1000,
            '万' => {
                total = total.checked_add((section + digit.unwrap_or(0)).checked_mul(10_000)?)?;
                section = 0;
                digit = None;
                continue;
            }
            _ => {
                digit = Some(cn_digit(c)?);
                continue;
            }
        };
        // A bare 十 at the start means 一十.
        section = section.checked_add(digit.unwrap_or(1).checked_mul(unit)?)?;
        digit = None;
    }
    total.checked_add(section + digit.unwrap_or(0))
}

/// Shrinks `[start, end)` to exclude surrounding whitespace; `None` if nothing is left.
pub(crate) fn trim_span(chars: &[char], start: usize, end: usize) -> Option<(usize, usize)> {
    let s = (start..end).find(|&i| !chars[i].is_whitespace())?;
    let e = (s..end).rev().find(|&i| !chars[i].is_whitespace())? + 1;
    Some((s, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numbers() {
        for (s, n) in [
            ("12", 12),
            ("十", 10),
            ("十二", 12),
            ("二十", 20),
            ("二十一", 21),
            ("一百", 100),
            ("一百零五", 105),
            ("一百一十", 110),
            ("两千零二十四", 2024),
            ("二〇二四", 2024),
            ("二○二三", 2023),
            ("三十一", 31),
        ] {
            assert_eq!(parse_number(s), Some(n), "{s}");
        }
        assert_eq!(parse_number("甲"), None);
    }

    #[test]
    fn normalizes_full_width() {
        assert_eq!(normalize("（１２）ＡＢ　，"), "(12)AB ,");
    }
}
