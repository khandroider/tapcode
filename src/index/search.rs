//! Fuzzy ranking for Go to File / Search Everywhere.
// ponytail: simple subsequence scorer; swap for `nucleo` if ranking feels off.

/// Higher is better. `None` when `query` is not a subsequence of `cand`.
pub fn fuzzy_score(query: &str, cand: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();
    let c: Vec<char> = cand.chars().collect();
    let cl: Vec<char> = c.iter().flat_map(|c| c.to_lowercase()).collect();
    let mut score = 0i32;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (i, ch) in cl.iter().enumerate() {
        if qi < q.len() && *ch == q[qi] {
            let boundary = i == 0 || matches!(c[i - 1], '/' | '_' | '-' | '.' | ' ') || (c[i].is_uppercase() && c[i - 1].is_lowercase());
            score += 10;
            if boundary {
                score += 20;
            }
            if last == Some(i - 1) {
                score += 15;
            }
            if let Some(l) = last {
                score -= ((i - l - 1) as i32).min(20);
            }
            last = Some(i);
            qi += 1;
        }
    }
    if qi < q.len() {
        return None;
    }
    // Prefer matches that land in the file name and shorter candidates.
    if let Some(slash) = cand.rfind('/')
        && last.is_some_and(|l| l > slash)
    {
        score += 30;
    }
    score -= (c.len() as i32) / 8;
    Some(score)
}

pub fn rank<'a>(query: &str, cands: impl Iterator<Item = &'a str>, limit: usize) -> Vec<(&'a str, i32)> {
    let mut v: Vec<(&str, i32)> = cands.filter_map(|c| fuzzy_score(query, c).map(|s| (c, s))).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.len().cmp(&b.0.len())));
    v.truncate(limit);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_prefers_boundaries_and_filename() {
        let files = [
            "app/Models/User.php",
            "app/Http/Controllers/UserController.php",
            "resources/views/users/index.blade.php",
            "tests/Unit/UsrTest.php",
        ];
        let r = rank("user", files.iter().copied(), 10);
        assert_eq!(r[0].0, "app/Models/User.php");
        assert!(r.iter().all(|(p, _)| *p != "tests/Unit/UsrTest.php"));
        let r = rank("uc", files.iter().copied(), 10);
        assert_eq!(r[0].0, "app/Http/Controllers/UserController.php");
        assert!(fuzzy_score("zzz", "abc").is_none());
    }
}
