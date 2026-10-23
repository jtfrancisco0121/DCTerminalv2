//! A4: check output trimmed for reading (and for an agent): ANSI codes and
//! progress redraws stripped, then the head, the tail and the error lines
//! between them, with a note of what was left out.

/// Removes CSI / OSC escape sequences and other control characters except
/// newline and tab. A carriage return keeps only what was drawn last on
/// that line (progress bars).
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut line = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                Some('[') => {
                    chars.next();
                    // Parameters and intermediates, then one final byte.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    continue;
                }
                line.clear();
            }
            '\n' => {
                out.push_str(line.trim_end());
                out.push('\n');
                line.clear();
            }
            '\t' => line.push(c),
            c if c.is_control() => {}
            c => line.push(c),
        }
    }
    out.push_str(line.trim_end());
    out
}

fn is_error_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "error",
        "fail",
        "panic",
        "exception",
        "traceback",
        "assert",
        "✗",
        "✕",
    ]
    .iter()
    .any(|w| lower.contains(w))
}

pub const HEAD: usize = 40;
pub const TAIL: usize = 80;
pub const MAX_ERRORS: usize = 60;
pub const MAX_LINE: usize = 400;

/// Head + error lines + tail; consecutive duplicate lines folded.
pub fn trim_output(text: &str, head: usize, tail: usize) -> String {
    let clean = strip_ansi(text);
    let mut lines: Vec<String> = Vec::new();
    let mut repeats = 0usize;
    for line in clean.lines() {
        let line: String = if line.chars().count() > MAX_LINE {
            format!("{}…", line.chars().take(MAX_LINE).collect::<String>())
        } else {
            line.to_string()
        };
        if lines.last() == Some(&line) {
            repeats += 1;
            continue;
        }
        if repeats > 0 {
            lines.push(format!("  (same line {repeats} more times)"));
            repeats = 0;
        }
        lines.push(line);
    }
    if repeats > 0 {
        lines.push(format!("  (same line {repeats} more times)"));
    }
    if lines.len() <= head + tail {
        return lines.join("\n");
    }
    let middle = &lines[head..lines.len() - tail];
    let errors: Vec<&String> = middle.iter().filter(|l| is_error_line(l)).collect();
    let shown = errors.len().min(MAX_ERRORS);
    let mut out: Vec<String> = lines[..head].to_vec();
    out.push(format!(
        "… {} lines left out{} …",
        middle.len() - shown,
        if shown > 0 {
            format!("; {shown} error lines kept")
        } else {
            String::new()
        }
    ));
    out.extend(errors.into_iter().take(MAX_ERRORS).cloned());
    if shown > 0 {
        out.push("…".into());
    }
    out.extend(lines[lines.len() - tail..].iter().cloned());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_codes_and_progress_redraws_are_stripped() {
        let raw = "\u{1b}[32mok\u{1b}[0m 1\n10%\r50%\r100% done\n\u{1b}]0;title\u{7}tail\r\nend";
        assert_eq!(strip_ansi(raw), "ok 1\n100% done\ntail\nend");
    }

    #[test]
    fn long_output_keeps_head_tail_and_error_lines() {
        let mut text = String::new();
        for n in 0..500 {
            text.push_str(&format!("line {n}\n"));
            if n == 250 {
                text.push_str("error[E0308]: mismatched types\n");
            }
        }
        let out = trim_output(&text, 5, 5);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "line 0");
        assert_eq!(lines[4], "line 4");
        assert!(
            lines[5].contains("lines left out; 1 error lines kept"),
            "{}",
            lines[5]
        );
        assert_eq!(lines[6], "error[E0308]: mismatched types");
        assert_eq!(*lines.last().unwrap(), "line 499");
        assert_eq!(lines.len(), 5 + 1 + 1 + 1 + 5);
        let repeated = "x\n".repeat(50);
        assert_eq!(
            trim_output(&repeated, 5, 5),
            "x\n  (same line 49 more times)"
        );
        assert_eq!(trim_output("short\n", 5, 5), "short");
    }
}
