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

/// Error and warning lines are never cut by [`compact_text`].
fn is_keep_line(line: &str) -> bool {
    is_error_line(line) || line.to_ascii_lowercase().contains("warn")
}

/// The line with every run of digits folded to `#`, so "Compiling a v0.1.2"
/// and "Compiling b v0.1.3", or two progress lines, count as the same shape.
fn shape(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_digits = false;
    for c in line.trim().chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(c);
        }
    }
    out
}

/// What the paste bar shows and pastes.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Compacted {
    pub text: String,
    pub lines_before: usize,
    pub lines_after: usize,
}

/// A big paste (a build or test log) made smaller to read or to hand to an
/// agent: ANSI and progress redraws stripped, runs of lines of the same shape
/// folded to the first one and a note, and, when it is still long, the head and
/// the tail kept with a note of what was left out between them. Error and
/// warning lines are never cut, and every cut leaves a note.
pub fn compact_text(text: &str) -> Compacted {
    let clean = strip_ansi(text);
    let before = clean.lines().count();
    let mut folded: Vec<String> = Vec::new();
    let mut run: Option<(String, usize)> = None; // (shape, extra lines folded)
    let flush = |folded: &mut Vec<String>, run: &mut Option<(String, usize)>| {
        if let Some((_, extra)) = run.take() {
            if extra > 0 {
                folded.push(format!("  (same kind of line {extra} more times)"));
            }
        }
    };
    for line in clean.lines() {
        let line: String = if line.chars().count() > MAX_LINE {
            format!("{}…", line.chars().take(MAX_LINE).collect::<String>())
        } else {
            line.to_string()
        };
        let keep = is_keep_line(&line);
        let this = shape(&line);
        match run.as_mut() {
            Some((prev, extra)) if !keep && !this.is_empty() && *prev == this => *extra += 1,
            _ => {
                flush(&mut folded, &mut run);
                folded.push(line);
                run = Some((if keep { String::new() } else { this }, 0));
            }
        }
    }
    flush(&mut folded, &mut run);
    let lines = if folded.len() > HEAD + TAIL {
        let middle = &folded[HEAD..folded.len() - TAIL];
        let kept: Vec<&String> = middle
            .iter()
            .filter(|l| is_keep_line(l) || l.starts_with("  (same kind of line"))
            .collect();
        let cut = middle.len() - kept.iter().filter(|l| is_keep_line(l)).count();
        let mut out: Vec<String> = folded[..HEAD].to_vec();
        out.push(format!(
            "… {cut} lines left out; error and warning lines kept …"
        ));
        out.extend(kept.into_iter().filter(|l| is_keep_line(l)).cloned());
        out.push("…".into());
        out.extend(folded[folded.len() - TAIL..].iter().cloned());
        out
    } else {
        folded
    };
    Compacted {
        lines_after: lines.len(),
        text: lines.join("\n"),
        lines_before: before,
    }
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

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../fixtures/paste")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn real_shaped_logs_keep_every_error_and_warning_line_and_shrink() {
        for name in ["cargo.txt", "npm.txt", "pytest.txt"] {
            let raw = fixture(name);
            let out = compact_text(&raw);
            for line in raw.lines().filter(|l| is_keep_line(l)) {
                assert!(out.text.contains(line.trim_end()), "{name}: lost `{line}`");
            }
            assert!(out.lines_after * 3 < out.lines_before, "{name}: {out:?}");
            assert!(
                out.text.contains("left out") || out.text.contains("same kind of line"),
                "{name}: no cut note"
            );
        }
    }

    #[test]
    fn repeats_are_folded_with_a_note_and_the_first_line_stays() {
        let raw: String = (0..50)
            .map(|i| format!("Compiling dep-{i} v1.{i}.0\n"))
            .collect();
        let out = compact_text(&raw);
        assert!(out.text.starts_with("Compiling dep-0 v1.0.0"));
        assert!(out.text.contains("(same kind of line 49 more times)"));
        assert_eq!(out.lines_before, 50);
    }

    #[test]
    fn an_error_line_inside_a_repeating_run_ends_the_run_and_is_kept() {
        let mut raw = String::new();
        for i in 0..30 {
            raw.push_str(&format!("step {i} ok\n"));
        }
        raw.push_str("step 30 failed: boom\n");
        for i in 31..60 {
            raw.push_str(&format!("step {i} ok\n"));
        }
        let out = compact_text(&raw);
        assert!(out.text.contains("step 30 failed: boom"));
    }
}
