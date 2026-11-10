//! Three-way content analysis. Ambiguous edits require an explicit current/result/delete choice.
use cowork_contracts::{AndroidMergeFileReview, AndroidMergeFileStatus};

pub(super) const MAX_MERGE_TEXT_BYTES: usize = 1024 * 1024;
pub(super) struct Analysis {
    pub review: AndroidMergeFileReview,
    pub merged: Option<Vec<u8>>,
}
struct Change<'a> {
    start: usize,
    end: usize,
    replacement: Vec<&'a str>,
}
fn change<'a>(base: &[&str], next: &[&'a str]) -> Change<'a> {
    let mut start = 0;
    while start < base.len().min(next.len()) && base[start] == next[start] {
        start += 1;
    }
    let mut trailing = 0;
    while trailing < base.len() - start
        && trailing < next.len() - start
        && base[base.len() - 1 - trailing] == next[next.len() - 1 - trailing]
    {
        trailing += 1;
    }
    Change {
        start,
        end: base.len() - trailing,
        replacement: next[start..next.len() - trailing].to_vec(),
    }
}
fn text(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > MAX_MERGE_TEXT_BYTES || bytes.contains(&0) {
        return None;
    }
    std::str::from_utf8(bytes).ok()
}
/// Disjoint original line ranges merge without guessing. Multiple edits within an overlapping
/// range conservatively remain a conflict, retaining both complete original versions.
fn merge_text(base: &str, current: &str, result: &str) -> Option<Vec<u8>> {
    if current == result {
        return Some(current.as_bytes().to_vec());
    }
    if current == base {
        return Some(result.as_bytes().to_vec());
    }
    if result == base {
        return Some(current.as_bytes().to_vec());
    }
    let b: Vec<_> = base.split_inclusive('\n').collect();
    let c: Vec<_> = current.split_inclusive('\n').collect();
    let r: Vec<_> = result.split_inclusive('\n').collect();
    if b.len() > 100_000 || c.len() > 100_000 || r.len() > 100_000 {
        return None;
    }
    let c = change(&b, &c);
    let r = change(&b, &r);
    if c.start == c.end && r.start == r.end && c.start == r.start {
        return None;
    }
    let (left, right) = if c.end <= r.start {
        (&c, &r)
    } else if r.end <= c.start {
        (&r, &c)
    } else {
        return None;
    };
    let mut out = String::new();
    for line in b[..left.start]
        .iter()
        .chain(left.replacement.iter())
        .chain(b[left.end..right.start].iter())
        .chain(right.replacement.iter())
        .chain(b[right.end..].iter())
    {
        out.push_str(line);
        if out.len() > 2 * MAX_MERGE_TEXT_BYTES {
            return None;
        }
    }
    Some(out.into_bytes())
}
pub(super) fn analyze(
    path: &str,
    base: Option<&str>,
    current: Option<&str>,
    result: Option<&str>,
    bytes: Option<(&[u8], &[u8], &[u8])>,
) -> Analysis {
    use AndroidMergeFileStatus::*;
    let mut merged = None;
    let mut preview = None;
    let status = if current == result {
        if current == base {
            Unchanged
        } else if current.is_none() {
            Deleted
        } else if base.is_none() {
            Added
        } else {
            IdenticalChange
        }
    } else if current == base {
        if result.is_none() {
            Deleted
        } else {
            ResultOnly
        }
    } else if result == base {
        if current.is_none() {
            Deleted
        } else {
            CurrentOnly
        }
    } else if let Some((b, c, r)) = bytes {
        if let (Some(b), Some(c), Some(r)) = (text(b), text(c), text(r)) {
            merged = merge_text(b, c, r);
            if merged.is_some() {
                AutoMerged
            } else {
                preview = Some(format!(
                    "BASE\n{}\nCURRENT\n{}\nRESULT\n{}",
                    b.chars().take(1200).collect::<String>(),
                    c.chars().take(1200).collect::<String>(),
                    r.chars().take(1200).collect::<String>()
                ));
                TextConflict
            }
        } else {
            BinaryConflict
        }
    } else {
        BinaryConflict
    };
    Analysis {
        review: AndroidMergeFileReview {
            path: path.into(),
            renamed_from: None,
            status,
            base_digest: base.map(str::to_owned),
            current_digest: current.map(str::to_owned),
            result_digest: result.map(str::to_owned),
            auto_mergeable: !matches!(status, TextConflict | BinaryConflict),
            conflict_preview: preview,
        },
        merged,
    }
}
