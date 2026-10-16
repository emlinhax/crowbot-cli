//! The error for an id a catalog does not have, listing the ones it does.

/// "unknown {kind} `{id}`; choose one of: a (what a is), b"; a choice without a description
/// is listed bare.
pub fn unknown<'a>(
    kind: &str,
    id: &str,
    choices: impl IntoIterator<Item = (&'a str, Option<String>)>,
) -> anyhow::Error {
    let listed: Vec<String> = choices
        .into_iter()
        .map(|(id, about)| match about {
            Some(about) => format!("{id} ({about})"),
            None => id.to_owned(),
        })
        .collect();
    anyhow::anyhow!(
        "unknown {kind} `{id}`; choose one of: {}",
        listed.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_every_choice_with_its_description() {
        let err = unknown(
            "mode",
            "fast",
            [("plan", Some("reads".into())), ("auto", None)],
        );
        assert_eq!(
            err.to_string(),
            "unknown mode `fast`; choose one of: plan (reads), auto"
        );
    }
}
