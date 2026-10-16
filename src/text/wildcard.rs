//! Matching text against a pattern of `*` and `?`, as permission rules and lists of names do.

/// `*` matches any run of characters (slashes included), `?` exactly one.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ti));
            pi += 1;
        } else if let Some((sp, st)) = star {
            pi = sp + 1;
            ti = st + 1;
            star = Some((sp, st + 1));
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcards() {
        assert!(wildcard("*", ""));
        assert!(wildcard("git *", "git commit -m x"));
        assert!(!wildcard("git *", "gitk"));
        assert!(wildcard("src/*.rs", "src/a/b.rs"));
        assert!(wildcard("?.env", "a.env"));
        assert!(!wildcard("*.env", "a.env.example"));
        assert!(wildcard("a*b*c", "aXbYc"));
    }
}
