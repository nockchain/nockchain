//! Clipboard helper utilities for the TUI.

/// Copy text to clipboard using OSC 52 escape sequence.
///
/// This is terminal-agnostic and works across SSH, tmux, etc.
/// The terminal must support OSC 52 for this to work.
pub fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    use std::io::Write;

    let encoded = base64_encode(text.as_bytes());
    let escape_sequence = format!("\x1b]52;c;{}\x07", encoded);

    std::io::stdout().write_all(escape_sequence.as_bytes())?;
    std::io::stdout().flush()?;

    Ok(())
}

/// Base64 encode bytes (simple implementation).
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    let mut i = 0;

    while i + 2 < input.len() {
        let b1 = input[i];
        let b2 = input[i + 1];
        let b3 = input[i + 2];

        result.push(CHARS[(b1 >> 2) as usize] as char);
        result.push(CHARS[(((b1 & 0x03) << 4) | (b2 >> 4)) as usize] as char);
        result.push(CHARS[(((b2 & 0x0f) << 2) | (b3 >> 6)) as usize] as char);
        result.push(CHARS[(b3 & 0x3f) as usize] as char);

        i += 3;
    }

    // Handle remaining bytes
    match input.len() - i {
        1 => {
            let b1 = input[i];
            result.push(CHARS[(b1 >> 2) as usize] as char);
            result.push(CHARS[((b1 & 0x03) << 4) as usize] as char);
            result.push('=');
            result.push('=');
        }
        2 => {
            let b1 = input[i];
            let b2 = input[i + 1];
            result.push(CHARS[(b1 >> 2) as usize] as char);
            result.push(CHARS[(((b1 & 0x03) << 4) | (b2 >> 4)) as usize] as char);
            result.push(CHARS[((b2 & 0x0f) << 2) as usize] as char);
            result.push('=');
        }
        _ => {}
    }

    result
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"test"), "dGVzdA==");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
    }
}
