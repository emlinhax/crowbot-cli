//! One value or a list of them, so a tool takes a single target or a batch in one call, and how a
//! batch's items are run and their results joined into one reply.

use serde::Deserialize;

use crate::text::template::fill;

#[derive(Deserialize)]
#[serde(untagged)]
pub enum Many<T> {
    One(T),
    Set(Vec<T>),
}

impl<T> Many<T> {
    pub fn all(&self) -> &[T] {
        match self {
            Self::One(item) => std::slice::from_ref(item),
            Self::Set(items) => items,
        }
    }

    /// The items, at most `max` of them.
    pub fn capped(&self, max: usize) -> &[T] {
        let all = self.all();
        &all[..all.len().min(max)]
    }
}

/// An id or search word, which the model may send as a string or a bare number.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum Id {
    Str(String),
    Int(i64),
}

impl Id {
    pub fn text(&self) -> String {
        match self {
            Self::Str(s) => s.clone(),
            Self::Int(n) => n.to_string(),
        }
    }
}

/// Run `each` over the items at once and stitch the results, labelling and inlining per-item
/// errors so one bad item does not sink the batch.
pub async fn batch<F, Fut>(items: Vec<String>, label: impl Fn(&str) -> String, each: F) -> String
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<String>>,
{
    let each = &each;
    let runs = items
        .into_iter()
        .map(move |item| async move { (item.clone(), each(item).await) });
    let parts = futures_util::future::join_all(runs)
        .await
        .into_iter()
        .map(|(item, result)| {
            let text = result.unwrap_or_else(|e| {
                fill(
                    &super::text().item_failed,
                    &[("item", &item), ("error", &format!("{e:#}"))],
                )
            });
            (label(&item), text)
        })
        .collect();
    join(parts)
}

/// Each result under its label, between rules; a single result stands alone, unlabelled.
pub fn join(parts: Vec<(String, String)>) -> String {
    let solo = parts.len() == 1;
    let mut out = String::new();
    for (label, text) in parts {
        if !out.is_empty() {
            out.push_str("\n\n───\n\n");
        }
        if !solo {
            out.push_str(&label);
            out.push('\n');
        }
        out.push_str(&text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_batch_labels_each_and_inlines_a_failure() {
        let out = batch(
            vec!["a".into(), "b".into()],
            |x| format!("[{x}]"),
            |x| async move {
                if x == "b" {
                    anyhow::bail!("nope")
                } else {
                    Ok(format!("ok {x}"))
                }
            },
        )
        .await;
        assert!(out.contains("[a]\nok a"), "{out}");
        assert!(out.contains("[b]\ncould not read b: nope"), "{out}");
        assert!(out.contains("───"), "{out}");

        // A single item carries no label or divider.
        let solo = batch(
            vec!["a".into()],
            |x| format!("[{x}]"),
            |x| async move { Ok(format!("ok {x}")) },
        )
        .await;
        assert_eq!(solo, "ok a");
    }
}
