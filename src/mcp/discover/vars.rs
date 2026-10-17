//! `${NAME}`, `${NAME:-default}` and `${env:NAME}` in another app's server entry, filled from the
//! environment as Claude Code and VS Code fill them.

use crate::mcp::config::Reach;

/// `reach` with its variables filled; `None` when one cannot be: unset with no default, or
/// another app's own (`${input:…}`, `${CLAUDE_PLUGIN_ROOT}` outside that app).
pub fn filled(reach: Reach, var: &dyn Fn(&str) -> Option<String>) -> Option<Reach> {
    let one = |text: &String| fill(text, var);
    Some(match reach {
        Reach::Program { command, args, env } => Reach::Program {
            command: one(&command)?,
            args: args.iter().map(one).collect::<Option<_>>()?,
            env: env
                .iter()
                .map(|(k, v)| Some((k.clone(), one(v)?)))
                .collect::<Option<_>>()?,
        },
        Reach::Url { url, headers } => Reach::Url {
            url: one(&url)?,
            headers: headers
                .iter()
                .map(|(k, v)| Some((k.clone(), one(v)?)))
                .collect::<Option<_>>()?,
        },
    })
}

fn fill(text: &str, var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let inner_and_after = &rest[start + 2..];
        let end = inner_and_after.find('}')?;
        let inner = &inner_and_after[..end];
        let inner = inner.strip_prefix("env:").unwrap_or(inner);
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        if name.contains(':') {
            return None;
        }
        out.push_str(&var(name).or(default.map(str::to_owned))?);
        rest = &inner_and_after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_are_filled_or_the_server_is_left_out() {
        let var = |name: &str| (name == "TOKEN").then(|| "t0k".to_owned());
        assert_eq!(fill("Bearer ${TOKEN}", &var).as_deref(), Some("Bearer t0k"));
        assert_eq!(fill("${env:TOKEN}", &var).as_deref(), Some("t0k"));
        assert_eq!(fill("${PORT:-8080}", &var).as_deref(), Some("8080"));
        assert_eq!(fill("plain", &var).as_deref(), Some("plain"));
        assert_eq!(fill("${UNSET}", &var), None);
        assert_eq!(fill("${input:key}", &var), None);
        assert_eq!(fill("${TOKEN", &var), None);
    }
}
