//! Validators must be `const fn` so that the macro-generated `Type::check_literal(...)` can
//! evaluate them while the code is compiled.
//!
//! A string validator takes `&str` rather than `&String`, because a `String` cannot exist in a
//! `const` context. The same function then serves both paths: the literals written in the source
//! and the values arriving from the database, the environment and Telegram.
//!
//! Working on `&str` in a `const fn` means working on bytes — `chars()` is not const — so the ones
//! below walk `as_bytes()` themselves.

const PROMO_CODE_MIN_LENGTH: usize = 4;
const PROMO_CODE_MAX_LENGTH: usize = 16;

pub const fn ratio_range_validator(x: &f64) -> bool {
    *x >= 0.0 && *x <= 1.0
}

pub const fn ratio_range_validator_f32(x: &f32) -> bool {
    *x >= 0.0 && *x <= 1.0
}

/// Latin and Cyrillic letters, digits, `_` and `-`, between 4 and 16 characters.
pub const fn promo_code_validator(code: &str) -> bool {
    let bytes = code.as_bytes();
    let mut i = 0;
    let mut length = 0;
    while i < bytes.len() {
        let width = promo_code_char_width(bytes, i);
        if width == 0 {
            return false
        }
        i += width;
        length += 1;
    }
    length >= PROMO_CODE_MIN_LENGTH && length <= PROMO_CODE_MAX_LENGTH
}

/// How many bytes the character at `i` takes, or zero when it isn't one the alphabet allows. Every
/// non-ASCII letter here is two bytes in UTF-8, so the pairs are matched directly instead of
/// decoding a code point.
const fn promo_code_char_width(bytes: &[u8], i: usize) -> usize {
    let first = bytes[i];
    if first.is_ascii_alphanumeric() || first == b'_' || first == b'-' {
        return 1
    }
    if i + 1 >= bytes.len() {
        return 0
    }
    let second = bytes[i + 1];
    match first {
        // Ё, then А-Я and а-п
        0xD0 => if matches!(second, 0x81 | 0x90..=0xBF) { 2 } else { 0 },
        // р-я, then ё
        0xD1 => if matches!(second, 0x80..=0x8F | 0x91) { 2 } else { 0 },
        _ => 0,
    }
}

pub const fn percentage_range_validator(x: &i32) -> bool {
    0 <= *x && *x <= 100
}

pub const fn percentage_range_validator_f64(x: &f64) -> bool {
    *x >= 0.0 && *x <= 100.0
}

#[cfg(test)]
mod test {
    use super::*;

    /// The alphabet came from a regular expression and is matched byte by byte now, so every
    /// letter it has to accept is checked one at a time.
    #[test]
    fn every_letter_of_the_alphabet_is_accepted() {
        let alphabet = ('a'..='z')
            .chain('A'..='Z')
            .chain('0'..='9')
            .chain('а'..='я')
            .chain('А'..='Я')
            .chain(['ё', 'Ё', '_', '-']);
        for letter in alphabet {
            let code = format!("aaa{letter}");
            assert!(promo_code_validator(&code), "{letter} belongs to the alphabet");
        }
    }

    #[test]
    fn anything_outside_the_alphabet_is_refused() {
        for code in ["promo!", "promo code", "promo—code", "прομο", "코드코드"] {
            assert!(!promo_code_validator(code), "{code} does not belong to the alphabet");
        }
    }

    /// A Cyrillic letter is two bytes, so counting bytes would refuse a code of legal length.
    #[test]
    fn a_promo_code_is_measured_in_characters() {
        assert!(promo_code_validator("абвгдеёжзийклмно"), "16 letters is the limit");
        assert!(!promo_code_validator("абвгдеёжзийклмноп"), "17 is past it");
        assert!(promo_code_validator("абвг"), "4 letters is the minimum");
        assert!(!promo_code_validator("абв"), "3 is below it");
        assert!(!promo_code_validator(""));
    }

}
