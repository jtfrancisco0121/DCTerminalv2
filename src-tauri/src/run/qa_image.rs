//! `run_qa_image`: one QA screenshot of a run, for the run panel (docs/launch.md, QA). The page names a run folder
//! and a file name; the app returns `<run>/reviews/qa/<name>.png` as a `data:` URL. The run folder must be this app's
//! (or an adopted one), the name is a plain file stem, no link is followed, the file must be a PNG of at most 5 MB.

use std::io::Read;
use std::path::Path;
use tauri::AppHandle;

/// A screenshot over this is refused (the driver caps them at the same size).
pub const MAX_BYTES: u64 = 5 * 1024 * 1024;
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// A file stem the QA driver may write: letters, digits, dot, dash, underscore; starts with a letter or digit.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 81
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        && !name.contains("..")
}

/// A regular file (never a link) opened without following a link at the last component.
fn open_regular(path: &Path) -> Result<std::fs::File, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|_| "image unavailable".to_string())?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err("image unavailable".into());
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(libc::O_NOFOLLOW);
    }
    opts.open(path).map_err(|_| "image unavailable".to_string())
}

/// The `data:image/png;base64,…` URL of `<run_dir>/reviews/qa/<name>.png`.
pub fn qa_image_data_url(run_dir: &Path, name: &str) -> Result<String, String> {
    use base64::Engine;
    if !valid_name(name) {
        return Err("not a QA image name".into());
    }
    let mut dir = run_dir.to_path_buf();
    for part in ["reviews", "qa"] {
        dir.push(part);
        let meta = std::fs::symlink_metadata(&dir).map_err(|_| "image unavailable".to_string())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err("image unavailable".into());
        }
    }
    let file = open_regular(&dir.join(format!("{name}.png")))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "image unavailable".to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("image is over 5 MB".into());
    }
    if !bytes.starts_with(&PNG_SIGNATURE) {
        return Err("not a PNG".into());
    }
    Ok(format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// The same, for a run folder the page named: it must be one of this app's runs (or an adopted one).
pub fn qa_image_in(data: &Path, run_dir: &str, name: &str) -> Result<String, String> {
    let dir = super::checked_run_dir(data, run_dir)?;
    qa_image_data_url(&dir, name)
}

#[tauri::command]
pub async fn run_qa_image(app: AppHandle, run_dir: String, name: String) -> Result<String, String> {
    super::blocking(move || qa_image_in(&super::data_dir(&app), &run_dir, &name)).await
}

/// One criterion row of a QA result file (`reviews/PR<n>-qa<r>.md`): `S1.1 | PASS | what was seen | reviews/qa/S1.1.png`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QaCriterion {
    pub id: String,
    /// `PASS`, `FAIL` or `ERROR`.
    pub result: String,
    pub text: String,
    /// The screenshot's file stem, for [`qa_image_data_url`]; None when the row names none.
    pub image: Option<String>,
}

fn criterion_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix('S') else {
        return false;
    };
    let Some((story, k)) = rest.split_once('.') else {
        return false;
    };
    !story.is_empty()
        && !k.is_empty()
        && story.chars().all(|c| c.is_ascii_digit())
        && k.chars().all(|c| c.is_ascii_digit())
}

/// The rows of a QA result file, failed first (then errors, then passes), at most 50. Read-only parsing.
pub fn parse_qa_rows(text: &str) -> Vec<QaCriterion> {
    let mut rows: Vec<QaCriterion> = text
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim().trim_start_matches(['-', '*']).trim();
            let parts: Vec<&str> = line.split('|').map(str::trim).collect();
            if parts.len() < 3 || !criterion_id(parts[0]) {
                return None;
            }
            let result = parts[1];
            if !matches!(result, "PASS" | "FAIL" | "ERROR") {
                return None;
            }
            let image = (parts.len() >= 4)
                .then(|| parts[parts.len() - 1])
                .and_then(|p| p.strip_prefix("reviews/qa/"))
                .and_then(|p| p.strip_suffix(".png"))
                .filter(|stem| valid_name(stem))
                .map(str::to_string);
            let middle = if image.is_some() {
                &parts[2..parts.len() - 1]
            } else {
                &parts[2..]
            };
            Some(QaCriterion {
                id: parts[0].to_string(),
                result: result.to_string(),
                text: middle.join(" | ").chars().take(300).collect(),
                image,
            })
        })
        .take(50)
        .collect();
    rows.sort_by_key(|r| match r.result.as_str() {
        "FAIL" => 0,
        "ERROR" => 1,
        _ => 2,
    });
    rows
}

/// The rows of the QA file a PR's `prs[].qa.path` names; only `reviews/PR<n>-qa<r>.md` of this run, no link followed,
/// at most 200 KB. Empty when the file is missing or unusable.
pub fn read_qa_rows(run_dir: &Path, pr_id: &str, rel: &str) -> Vec<QaCriterion> {
    let expected_prefix = format!("reviews/{pr_id}-qa");
    let Some(rest) = rel.strip_prefix(&expected_prefix) else {
        return Vec::new();
    };
    let Some(round) = rest.strip_suffix(".md") else {
        return Vec::new();
    };
    if round.len() != 1 || !round.chars().all(|c| c.is_ascii_digit()) {
        return Vec::new();
    }
    let reviews = run_dir.join("reviews");
    if std::fs::symlink_metadata(&reviews).is_ok_and(|m| m.file_type().is_symlink()) {
        return Vec::new();
    }
    let Ok(file) = open_regular(&run_dir.join(rel)) else {
        return Vec::new();
    };
    let mut text = String::new();
    if file.take(200_000).read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    parse_qa_rows(&text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    fn png() -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend_from_slice(b"rest");
        bytes
    }

    fn run_with(dir: &TempDir, file: &str, bytes: &[u8]) -> std::path::PathBuf {
        let run = dir.join("runs").join("r-20261106-100000-abcd");
        std::fs::create_dir_all(run.join("reviews/qa")).unwrap();
        std::fs::write(run.join("reviews/qa").join(file), bytes).unwrap();
        run
    }

    #[test]
    fn a_png_of_the_run_comes_back_as_a_data_url() {
        let dir = TempDir::new("qa_image_ok");
        let run = run_with(&dir, "S1.1.png", &png());
        let url = qa_image_data_url(&run, "S1.1").unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
    }

    #[test]
    fn a_name_with_a_separator_or_dots_is_refused() {
        let dir = TempDir::new("qa_image_name");
        let run = run_with(&dir, "S1.1.png", &png());
        for bad in [
            "../S1.1",
            "a/b",
            "..",
            "a\\b",
            "",
            ".hidden",
            "S1..1",
            "S1.1.png/",
        ] {
            assert!(qa_image_data_url(&run, bad).is_err(), "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_is_refused_for_the_file_and_for_the_folders() {
        let dir = TempDir::new("qa_image_link");
        let run = run_with(&dir, "real.png", &png());
        std::os::unix::fs::symlink(
            run.join("reviews/qa/real.png"),
            run.join("reviews/qa/link.png"),
        )
        .unwrap();
        assert!(qa_image_data_url(&run, "link").is_err());
        assert!(qa_image_data_url(&run, "real").is_ok());
        let other = dir.join("elsewhere");
        std::fs::create_dir_all(other.join("qa")).unwrap();
        std::fs::write(other.join("qa/real.png"), png()).unwrap();
        let run2 = dir.join("runs").join("r-20261106-100000-ffff");
        std::fs::create_dir_all(&run2).unwrap();
        std::os::unix::fs::symlink(&other, run2.join("reviews")).unwrap();
        assert!(qa_image_data_url(&run2, "real").is_err());
    }

    #[test]
    fn a_file_that_is_not_a_png_is_refused() {
        let dir = TempDir::new("qa_image_nopng");
        let run = run_with(&dir, "x.png", b"<html>not an image</html>");
        assert_eq!(qa_image_data_url(&run, "x").unwrap_err(), "not a PNG");
    }

    #[test]
    fn a_file_over_5_mb_is_refused_and_one_at_the_limit_is_not() {
        let dir = TempDir::new("qa_image_big");
        let mut big = png();
        big.resize(MAX_BYTES as usize + 1, 0);
        let run = run_with(&dir, "big.png", &big);
        assert_eq!(
            qa_image_data_url(&run, "big").unwrap_err(),
            "image is over 5 MB"
        );
        let mut ok = png();
        ok.resize(MAX_BYTES as usize, 0);
        std::fs::write(run.join("reviews/qa/edge.png"), &ok).unwrap();
        assert!(qa_image_data_url(&run, "edge").is_ok());
    }

    #[test]
    fn only_this_apps_run_folders_are_served() {
        let dir = TempDir::new("qa_image_other");
        let data = dir.join("data");
        let mine = data.join("runs").join("r-20261106-100000-abcd");
        std::fs::create_dir_all(mine.join("reviews/qa")).unwrap();
        std::fs::write(mine.join("reviews/qa/a.png"), png()).unwrap();
        assert!(qa_image_in(&data, &mine.display().to_string(), "a").is_ok());
        let stranger = dir.join("elsewhere").join("r-20261106-100000-9999");
        std::fs::create_dir_all(stranger.join("reviews/qa")).unwrap();
        std::fs::write(stranger.join("reviews/qa/a.png"), png()).unwrap();
        assert!(qa_image_in(&data, &stranger.display().to_string(), "a").is_err());
        assert!(qa_image_in(&data, "/definitely/not/here", "a").is_err());
    }

    #[test]
    fn the_qa_file_rows_are_parsed_failed_first() {
        let text = "QA: fail\nS1.1 | PASS | Start button visible | reviews/qa/S1.1.png\n\
                    S1.2 | FAIL | Planning not shown | reviews/qa/S1.2.png\n\
                    S2.1 | ERROR | page did not load\nnot a row\nS3.1 | MAYBE | x | y\n";
        let rows = parse_qa_rows(text);
        assert_eq!(
            rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["S1.2", "S2.1", "S1.1"]
        );
        assert_eq!(rows[0].image.as_deref(), Some("S1.2"));
        assert_eq!(rows[0].text, "Planning not shown");
        assert_eq!(rows[1].image, None);
        assert_eq!(rows[1].text, "page did not load");
    }

    #[test]
    fn only_the_named_qa_file_of_the_pr_is_read() {
        let dir = TempDir::new("qa_rows");
        let run = dir.join("r-1");
        std::fs::create_dir_all(run.join("reviews")).unwrap();
        std::fs::write(
            run.join("reviews/PR1-qa1.md"),
            "QA: pass\nS1.1 | PASS | ok | reviews/qa/S1.1.png\n",
        )
        .unwrap();
        assert_eq!(read_qa_rows(&run, "PR1", "reviews/PR1-qa1.md").len(), 1);
        assert!(read_qa_rows(&run, "PR2", "reviews/PR1-qa1.md").is_empty());
        assert!(read_qa_rows(&run, "PR1", "../x/PR1-qa1.md").is_empty());
        assert!(read_qa_rows(&run, "PR1", "reviews/PR1-qa12.md").is_empty());
        assert!(read_qa_rows(&run, "PR1", "reviews/PR1-qa2.md").is_empty());
    }
}
