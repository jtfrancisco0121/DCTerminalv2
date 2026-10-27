//! The secret scan of `plugin/scripts/memory.sh` (`SECRETS`, `secretish`,
//! `password_like`, `pathish`), ported rule for rule, so a ClickUp comment
//! the app posts on its own is held to the same bar as a memory write. A
//! hit names the kind of secret and the line, never the value.

use regex::{Captures, Regex};
use std::sync::OnceLock;

type Check = fn(&Captures) -> bool;

struct Rules {
    placeholder: Regex,
    dotted: Regex,
    ext: Regex,
    long_run: Regex,
    lower: Regex,
    upper: Regex,
    digit: Regex,
    pw_name: Regex,
    secrets: Vec<(&'static str, Regex, Option<Check>)>,
}

fn rules() -> &'static Rules {
    static RULES: OnceLock<Rules> = OnceLock::new();
    RULES.get_or_init(|| {
        let re = |p: &str| Regex::new(p).expect("secret scan regex");
        Rules {
            placeholder: re(concat!(
                r"(?i)^(x{3,}|\*+|\.{3}|<.*>|\$\{?\w*\}?|%\w*%|\{\{.*\}\}|changeme|change[_-]?me|example\w*|placeholder|your[_-]\w*|",
                r"none|null|nil|true|false|empty|dummy|redacted|todo|tbd|dev[_-]only\w*|not[_-]a[_-]secret\w*|secret|password)$"
            )),
            dotted: re(r"^[\w-]+(\.[\w-]+)+$"),
            ext: re(r"\.[A-Za-z][A-Za-z0-9]{0,4}$"),
            long_run: re(r"[A-Za-z0-9+/=_-]{20,}"),
            lower: re(r"[a-z]"),
            upper: re(r"[A-Z]"),
            digit: re(r"[0-9]"),
            pw_name: re(r"passw|pwd|secret|token|credential"),
            secrets: vec![
                ("a private key", re(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----"), None),
                ("an AWS access key id", re(r"\b(AKIA|ASIA|AGPA|AIDA|AROA)[0-9A-Z]{16}\b"), None),
                ("an AWS secret key", re(r#"(?i)aws_?secret_?access_?key["']?\s*[=:]\s*\S{8,}"#), None),
                ("a GitHub token", re(r"\b(gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})"), None),
                (
                    "an API key (sk-)",
                    re(r"\b(sk-(?:ant-|proj-|live-)?(?:api\d\d-)?[A-Za-z0-9_-]{20,})"),
                    Some(group1_secretish),
                ),
                (
                    "a token (pk_/sk_/rk_)",
                    re(r"\b((?:pk|sk|rk)_(?:live_|test_)?[A-Za-z0-9_]{12,})"),
                    Some(group1_secretish),
                ),
                ("a Slack token", re(r"\bxox[abposr]-[A-Za-z0-9-]{10,}"), None),
                ("a Google API key", re(r"\bAIza[0-9A-Za-z_-]{30,}"), None),
                ("a JWT", re(r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]+"), None),
                ("a password in a URL", re(r"[a-z][a-z0-9+.-]*://[^/\s:@]+:[^/\s@]{3,}@"), None),
                (
                    "a Bearer token",
                    re(r#"(?i)\bAuthorization["']?\s*[:=]\s*["']?Bearer\s+([A-Za-z0-9._~+/=-]{16,})"#),
                    Some(group1_secretish),
                ),
                (
                    "a webhook URL",
                    re(concat!(
                        r"(?i)\bhttps?://(?:hooks\.slack\.com/services/|discord(?:app)?\.com/api/webhooks/|outlook\.office\.com/webhook/|",
                        r"[a-z0-9-]+\.webhook\.office\.com/|hooks\.zapier\.com/hooks/catch/)[A-Za-z0-9]"
                    )),
                    None,
                ),
                (
                    "a secret assignment (.env / JSON / YAML style)",
                    re(concat!(
                        r#"(?i)(?:^|[\s"'{,(])(?:export\s+)?["']?[A-Za-z0-9_.-]*?"#,
                        r"(passw(?:or)?d|pwd|client[ _-]?secret|secret|token|credentials?|api[ _-]?key|private[ _-]?key|access[ _-]?key|",
                        r#"auth[ _-]?key|_key|key)[A-Za-z0-9_]*["']?\s*[=:]\s*["']?([^\s"'`<>,;]+)"#
                    )),
                    Some(assignment),
                ),
            ],
        }
    })
}

fn strip_quotes(v: &str) -> &str {
    v.trim_matches(|c| c == '"' || c == '\'' || c == '`')
}

/// A file path or dotted name rather than a secret.
fn pathish(v: &str) -> bool {
    let r = rules();
    if ["./", "../", "/", "~/", "src/"]
        .iter()
        .any(|p| v.starts_with(p))
    {
        return true;
    }
    if r.dotted.is_match(v) {
        return true;
    }
    if v.contains('/') {
        if r.ext.is_match(v) {
            return true;
        }
        return !r.long_run.is_match(v);
    }
    false
}

/// A token-shaped value: long, mixed character classes, high entropy, not a path.
pub fn secretish(v: &str) -> bool {
    let r = rules();
    let v = strip_quotes(v);
    let len = v.chars().count();
    if len < 16 || pathish(v) || r.placeholder.is_match(v) {
        return false;
    }
    let classes = [&r.lower, &r.upper, &r.digit]
        .iter()
        .filter(|rx| rx.is_match(v))
        .count();
    if classes < 2 {
        return false;
    }
    let mut counts = std::collections::HashMap::new();
    for c in v.chars() {
        *counts.entry(c).or_insert(0usize) += 1;
    }
    let n = len as f64;
    let entropy: f64 = counts
        .values()
        .map(|&k| {
            let p = k as f64 / n;
            -p * p.log2()
        })
        .sum();
    entropy >= 3.0
}

/// For password / secret / token / credential names: any real-looking value of 8+ characters.
fn password_like(v: &str) -> bool {
    let v = strip_quotes(v);
    v.chars().count() >= 8 && !rules().placeholder.is_match(v) && !pathish(v)
}

fn group1_secretish(m: &Captures) -> bool {
    m.get(1).is_some_and(|g| secretish(g.as_str()))
}

fn assignment(m: &Captures) -> bool {
    let name = m
        .get(1)
        .map(|g| g.as_str().to_lowercase())
        .unwrap_or_default();
    let value = m.get(2).map(|g| g.as_str()).unwrap_or("");
    if rules().pw_name.is_match(&name) {
        password_like(value)
    } else {
        secretish(value)
    }
}

/// (line number, kind) for every line that looks like a secret; never the value.
pub fn secret_hits(text: &str) -> Vec<(usize, &'static str)> {
    let r = rules();
    let mut hits = Vec::new();
    for (n, line) in text.split(['\n', '\r']).enumerate() {
        for (kind, rx, check) in &r.secrets {
            let hit = rx
                .captures_iter(line)
                .any(|m| check.is_none_or(|ok| ok(&m)));
            if hit {
                hits.push((n + 1, *kind));
                break;
            }
        }
    }
    hits
}

/// The first secret-looking line's kind, if any.
pub fn first_hit(text: &str) -> Option<&'static str> {
    secret_hits(text).first().map(|(_, kind)| *kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_looking_secrets_are_found_and_plain_text_is_not() {
        let hits = [
            "-----BEGIN RSA PRIVATE KEY-----",
            "key id AKIAIOSFODNN7EXAMPLE here",
            "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG",
            "token ghp_abcdefghijklmnopqrstuvwxyz0123",
            "use sk-ant-api03-Zx8Kq2Lm9Pw4Rt7Yb1Nc6Vd3",
            "ClickUp pk_81234567_Q2W3E4R5T6Y7U8I9O0PLKJHGF",
            "xoxb-1234567890-abcdefghij",
            "AIzaSyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abc",
            "https://bob:hunter22@example.com/x",
            "curl -H 'Authorization: Bearer 9fK2mQ7xL4pR8tW1zB6nC3vD' https://api.example.com",
            "https://hooks.slack.com/services/T0123ABCD/B0123ABCD/abcdEFGH1234",
            "DB_PASSWORD=s3cr3tPass!",
            "\"apiKey\": \"Zx8Kq2Lm9Pw4Rt7Yb1Nc6Vd3\"",
        ];
        for line in hits {
            assert!(first_hit(line).is_some(), "missed: {line}");
        }
        let clean = [
            "Done: S1 export invoices. PR https://github.com/o/r/pull/12",
            "password=changeme",
            "TOKEN=<your token>",
            "api_key: ${API_KEY}",
            "key: src/components/StartScreen.tsx",
            "secret = example_value",
            "sk_short",
            "The token is stored in the keychain.",
            "The Bearer token is rotated nightly.",
            "Authorization: Bearer <your token>",
            "Send Authorization: Bearer ${TOKEN} with each call",
            "see https://hooks.slack.com/ for how webhooks work",
            "monkey: banana",
        ];
        for line in clean {
            assert!(
                first_hit(line).is_none(),
                "false hit: {line} {:?}",
                secret_hits(line)
            );
        }
        let text = "fine\nDB_PASSWORD=s3cr3tPass!\nfine";
        assert_eq!(
            secret_hits(text),
            [(2, "a secret assignment (.env / JSON / YAML style)")]
        );
    }

    /// The same verdicts as memory.sh's own scanner, when python3 is here.
    #[test]
    fn verdicts_match_memory_sh() {
        let script = crate::run::launch::dev_repo_root().join("plugin/scripts/memory.sh");
        let Ok(text) = std::fs::read_to_string(&script) else {
            return;
        };
        let Some(py) = text
            .split("IFS= read -r -d '' PY <<'PY' || true\n")
            .nth(1)
            .and_then(|rest| rest.split("\nPY\n").next())
        else {
            return;
        };
        let lines = [
            "DB_PASSWORD=s3cr3tPass!",
            "password=changeme",
            "export GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123",
            "key: src/components/StartScreen.tsx",
            "auth_key = 9fK2mQ7xL4pR8tW1zB6nC3vD",
            "see https://bob:hunter22@example.com/x",
            "curl -H \"Authorization: Bearer 9fK2mQ7xL4pR8tW1zB6nC3vD\" https://api.example.com",
            "Authorization: Bearer <your token>",
            "The Bearer token is rotated nightly.",
            "https://hooks.slack.com/services/T0123ABCD/B0123ABCD/abcdEFGH1234",
            "post to https://discord.com/api/webhooks/123456789/abcdefghijkl",
            "https://discordapp.com/api/webhooks/123456789/abcdefghijkl",
            "https://outlook.office.com/webhook/abc-123@def/IncomingWebhook/xyz",
            "https://contoso.webhook.office.com/webhookb2/abc-123@def/IncomingWebhook/xyz",
            "https://hooks.zapier.com/hooks/catch/123456/abcdef/",
            "docs at https://hooks.slack.com/ explain webhooks",
            "pk_live_abcdefghijklmnop",
            "pk_live_Q2W3E4R5T6Y7U8I9O0PL",
            "client-secret: 'abc'",
            "\"token\": \"0123456789abcdefABCDEF\"",
            "nothing here at all",
        ];
        let probe = format!(
            "{}\nimport json as _j\nprint(_j.dumps([bool(secret_hits(l)) for l in {}]))\n",
            py.split("\ndef cmd_status").next().unwrap_or(py),
            serde_json::to_string(&lines).unwrap()
        );
        let Ok(out) = std::process::Command::new("python3")
            .arg("-c")
            .arg(&probe)
            .output()
        else {
            return;
        };
        if !out.status.success() {
            return;
        }
        let theirs: Vec<bool> = serde_json::from_slice(&out.stdout).unwrap();
        let ours: Vec<bool> = lines.iter().map(|l| first_hit(l).is_some()).collect();
        assert_eq!(ours, theirs, "{lines:?}");
    }
}
