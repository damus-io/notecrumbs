#[inline]
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let lower_bound = index.saturating_sub(3);
        let new_index = s.as_bytes()[lower_bound..=index]
            .iter()
            .rposition(|b| is_utf8_char_boundary(*b));

        // SAFETY: we know that the character boundary will be within four bytes
        unsafe { lower_bound + new_index.unwrap_unchecked() }
    }
}

#[inline]
fn is_utf8_char_boundary(c: u8) -> bool {
    // This is bit magic equivalent to: b < 128 || b >= 192
    (c as i8) >= -0x40
}

const ABBREV_SIZE: usize = 10;

pub fn abbrev_str(name: &str) -> String {
    if name.len() > ABBREV_SIZE {
        let closest = floor_char_boundary(name, ABBREV_SIZE);
        format!("{}...", &name[..closest])
    } else {
        name.to_owned()
    }
}

pub fn abbreviate(text: &str, len: usize) -> &str {
    let closest = floor_char_boundary(text, len);
    &text[..closest]
}
