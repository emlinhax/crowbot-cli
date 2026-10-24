//! Which subagent may change which files, while several work at once. Each subagent a `task` call
//! starts is a holder from its start to its end, with the scope the call gave it (patterns where
//! `*` matches anything and a plain directory covers what is under it). A change is refused when
//! another holder's scope covers the path, or when it lies outside a scoped subagent's own; the
//! `claim` tool widens a scope once nobody else holds the path, or waits for the holder to end.
//! The table is the frontend's run's, so every subagent of one prompt shares it.
// CEILING: only `edit`, `write` and `fs` are checked; a shell command that writes files is not
// seen. Upgrade: a git worktree per scoped subagent, merged back when it ends.
// CEILING: paths compare as text, so on a case-insensitive filesystem `Src/a.rs` and `src/a.rs`
// are two paths. Upgrade: fold case where the filesystem does.
// CEILING: whether two patterns overlap is judged from their literal heads, so `src/*.rs` and
// `src/lib.rs` overlap but so do `src/a*` and `src/b*`. Upgrade: a real glob intersection.

use std::sync::{LazyLock, Mutex};

use serde::Deserialize;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::data;
use crate::text::template::fill;
use crate::text::wildcard::wildcard;

static TEXT: LazyLock<Text> = data::load!("claims.toml");

/// The tools whose changes the table sees.
pub const CHANGING: &[&str] = &["edit", "write", "fs"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Text {
    roster: String,
    own_scope: String,
    no_scope: String,
    others: String,
    alone: String,
    holder: String,
    nothing_held: String,
    outside: String,
    held: String,
    granted: String,
    conflict: String,
    deadlock: String,
    not_subagent: String,
    cancelled: String,
    launch_conflict: String,
}

/// A subagent at work, and the files it holds.
#[derive(Clone, Debug)]
pub struct Holder {
    /// The `task` call that started it.
    pub call: String,
    pub kind: String,
    pub job: String,
    pub model: String,
    /// Empty: it holds nothing, and may change what no one else holds.
    pub scope: Vec<String>,
}

#[derive(Default)]
struct State {
    holders: Vec<Holder>,
    /// Who waits on whom, by call: a wait that would close a circle is refused.
    waits: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Table {
    state: Mutex<State>,
    released: Notify,
}

impl Table {
    /// Adds `holder` for its run's length; `Err` says whose scope its own overlaps.
    pub fn register(&self, holder: Holder) -> Result<(), String> {
        let mut state = self.lock();
        for pattern in &holder.scope {
            if let Some(other) = held_by(&state.holders, &holder.call, pattern) {
                return Err(said(&TEXT.launch_conflict, pattern, other));
            }
        }
        state.holders.push(holder);
        Ok(())
    }

    /// The subagent of `call` ended: what it held is free, and whoever waited is told.
    pub fn release(&self, call: &str) {
        let mut state = self.lock();
        state.holders.retain(|h| h.call != call);
        state.waits.retain(|(waiter, _)| waiter != call);
        drop(state);
        self.released.notify_waiters();
    }

    /// What the subagent of `call` is told as it starts: its scope, who else is at work, the rules.
    pub fn roster(&self, call: &str) -> String {
        let state = self.lock();
        let own = state.holders.iter().find(|h| h.call == call);
        let scope = match own.filter(|h| !h.scope.is_empty()) {
            Some(h) => fill(&TEXT.own_scope, &[("scope", &h.scope.join(", "))]),
            None => TEXT.no_scope.clone(),
        };
        let lines: Vec<String> = state
            .holders
            .iter()
            .filter(|h| h.call != call)
            .map(|h| {
                let scope = shown_scope(h);
                fill(
                    &TEXT.holder,
                    &[
                        ("kind", &h.kind),
                        ("model", &h.model),
                        ("job", &h.job),
                        ("scope", &scope),
                    ],
                )
            })
            .collect();
        let others = match lines.is_empty() {
            true => TEXT.alone.clone(),
            false => fill(&TEXT.others, &[("list", &lines.join("\n"))]),
        };
        fill(&TEXT.roster, &[("scope", &scope), ("others", &others)])
            .trim()
            .to_owned()
    }

    /// Whether `who` (a subagent's call, `None` for the frontend's run) may change `path`, a path
    /// as the project shows it.
    pub fn may_change(&self, who: Option<&str>, path: &str) -> Result<(), String> {
        let state = self.lock();
        if let Some(other) = held_by(&state.holders, who.unwrap_or_default(), path) {
            return Err(said(&TEXT.held, path, other));
        }
        let own = who.and_then(|call| state.holders.iter().find(|h| h.call == call));
        if let Some(own) = own.filter(|h| !h.scope.is_empty())
            && !own.scope.iter().any(|p| covers(p, path))
        {
            let scope = own.scope.join(", ");
            return Err(fill(&TEXT.outside, &[("path", path), ("scope", &scope)]));
        }
        Ok(())
    }

    /// Widens `who`'s scope by `patterns` once no one else holds them; with `wait`, waits for the
    /// holders to end, unless that would wait on a subagent waiting on `who`.
    pub async fn claim(
        &self,
        who: Option<&str>,
        patterns: &[String],
        wait: bool,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let Some(who) = who else {
            return Err(TEXT.not_subagent.clone());
        };
        loop {
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            // The lock is let go before the wait: a block, since a guard dropped by hand still
            // counts as held across the await.
            let pattern = {
                let mut state = self.lock();
                let blocked = patterns
                    .iter()
                    .find_map(|p| held_by(&state.holders, who, p).map(|h| (p.clone(), h.clone())));
                let Some((pattern, holder)) = blocked else {
                    if let Some(own) = state.holders.iter_mut().find(|h| h.call == who) {
                        own.scope.extend(patterns.iter().cloned());
                    }
                    return Ok(fill(&TEXT.granted, &[("paths", &patterns.join(", "))]));
                };
                if !wait {
                    return Err(said(&TEXT.conflict, &pattern, &holder));
                }
                if waits_on(&state.waits, &holder.call, who) {
                    return Err(said(&TEXT.deadlock, &pattern, &holder));
                }
                state.waits.push((who.to_owned(), holder.call.clone()));
                pattern
            };
            tokio::select! {
                () = &mut released => {}
                () = cancel.cancelled() => {
                    self.lock().waits.retain(|(w, _)| w != who);
                    return Err(fill(&TEXT.cancelled, &[("path", &pattern)]));
                }
            }
            self.lock().waits.retain(|(w, _)| w != who);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A path or pattern as the project shows paths: `/` separators, no leading `./`.
pub fn normalize(pattern: &str) -> String {
    let pattern = pattern.trim().replace('\\', "/");
    let pattern = pattern.trim_start_matches("./");
    pattern.trim_end_matches('/').to_owned()
}

/// What the agent the user works with hears from `claim`.
pub fn not_subagent() -> String {
    TEXT.not_subagent.clone()
}

/// The holder other than `call` whose scope overlaps `pattern`.
fn held_by<'a>(holders: &'a [Holder], call: &str, pattern: &str) -> Option<&'a Holder> {
    holders
        .iter()
        .filter(|h| h.call != call)
        .find(|h| h.scope.iter().any(|p| overlaps(p, pattern)))
}

/// Whether `from` waits, directly or through others, on `to`.
fn waits_on(waits: &[(String, String)], from: &str, to: &str) -> bool {
    let mut seen = vec![from.to_owned()];
    let mut next = vec![from.to_owned()];
    while let Some(at) = next.pop() {
        for (_, on) in waits.iter().filter(|(w, _)| *w == at) {
            if on == to {
                return true;
            }
            if !seen.contains(on) {
                seen.push(on.clone());
                next.push(on.clone());
            }
        }
    }
    false
}

/// Whether `pattern` covers `path`: a match, or a directory `path` is under.
fn covers(pattern: &str, path: &str) -> bool {
    let pattern = pattern.trim_end_matches('/');
    wildcard(pattern, path) || path.starts_with(&format!("{pattern}/"))
}

/// Whether some path could fall under both patterns, judged from where their wildcards start.
fn overlaps(a: &str, b: &str) -> bool {
    let (ha, hb) = (head(a), head(b));
    covers(a, hb) || covers(b, ha) || ha.starts_with(hb) || hb.starts_with(ha)
}

/// A pattern up to its first wildcard.
fn head(pattern: &str) -> &str {
    let end = pattern.find(['*', '?']).unwrap_or(pattern.len());
    pattern[..end].trim_end_matches('/')
}

fn shown_scope(holder: &Holder) -> String {
    match holder.scope.is_empty() {
        true => TEXT.nothing_held.clone(),
        false => holder.scope.join(", "),
    }
}

fn said(template: &str, path: &str, holder: &Holder) -> String {
    let scope = shown_scope(holder);
    fill(
        template,
        &[
            ("path", path),
            ("kind", &holder.kind),
            ("job", &holder.job),
            ("scope", &scope),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(call: &str, scope: &[&str]) -> Holder {
        Holder {
            call: call.into(),
            kind: format!("kind-{call}"),
            job: format!("job {call}"),
            model: "m".into(),
            scope: scope.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    fn paths(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_directory_covers_what_is_under_it_and_patterns_overlap_by_their_heads() {
        assert!(covers("src/parser", "src/parser/lex.rs"));
        assert!(!covers("src/parser", "src/parserx.rs"));
        assert!(covers("src/*.rs", "src/lib.rs"));
        assert!(overlaps("src/**", "src/lib.rs"));
        assert!(overlaps("src/lib.rs", "src/*.rs"));
        assert!(!overlaps("src/parser/**", "tests/a.rs"));
        assert!(!overlaps("src/a.rs", "src/b.rs"));
    }

    #[test]
    fn a_scoped_subagent_changes_only_its_scope_and_nobody_changes_what_another_holds() {
        let table = Table::default();
        table.register(holder("a", &["src/parser"])).unwrap();
        table.register(holder("b", &[])).unwrap();
        assert!(table.may_change(Some("a"), "src/parser/lex.rs").is_ok());
        let outside = table.may_change(Some("a"), "src/main.rs").unwrap_err();
        assert!(outside.contains("outside your scope"), "{outside}");
        // Unscoped: anything nobody holds, never what another holds.
        assert!(table.may_change(Some("b"), "src/main.rs").is_ok());
        let held = table
            .may_change(Some("b"), "src/parser/lex.rs")
            .unwrap_err();
        assert!(held.contains("kind-a") && held.contains("job a"), "{held}");
        // The frontend's run is held off too, until the holder ends.
        assert!(table.may_change(None, "src/parser/lex.rs").is_err());
        table.release("a");
        assert!(table.may_change(None, "src/parser/lex.rs").is_ok());
    }

    #[test]
    fn overlapping_scopes_are_refused_at_launch() {
        let table = Table::default();
        table.register(holder("a", &["src/**"])).unwrap();
        let err = table.register(holder("b", &["src/lib.rs"])).unwrap_err();
        assert!(
            err.contains("Not started") && err.contains("kind-a"),
            "{err}"
        );
        assert!(table.register(holder("c", &["docs"])).is_ok());
    }

    #[test]
    fn the_roster_names_the_others_and_the_scope() {
        let table = Table::default();
        table.register(holder("a", &["src/parser"])).unwrap();
        table.register(holder("b", &[])).unwrap();
        let a = table.roster("a");
        assert!(
            a.contains("Your scope, the files you may change: src/parser."),
            "{a}"
        );
        assert!(a.contains("kind-b (m), \"job b\": no files held"), "{a}");
        let b = table.roster("b");
        assert!(b.contains("You were given no scope"), "{b}");
        assert!(b.contains("\"job a\": src/parser"), "{b}");
    }

    #[tokio::test]
    async fn a_claim_is_granted_refused_or_waits_for_the_holder_to_end() {
        let table = Table::default();
        let cancel = CancellationToken::new();
        table.register(holder("a", &["src/a.rs"])).unwrap();
        table.register(holder("b", &["src/b.rs"])).unwrap();
        let granted = table
            .claim(Some("a"), &paths(&["src/c.rs"]), false, &cancel)
            .await;
        assert!(granted.unwrap().contains("src/c.rs"));
        assert!(table.may_change(Some("a"), "src/c.rs").is_ok());
        let refused = table
            .claim(Some("a"), &paths(&["src/b.rs"]), false, &cancel)
            .await;
        assert!(refused.unwrap_err().contains("kind-b"));
        assert!(
            table
                .claim(None, &paths(&["x"]), false, &cancel)
                .await
                .is_err()
        );

        // a waits for b; b then asking to wait for a is a circle, refused at once.
        let wanted = paths(&["src/b.rs"]);
        let waiting = table.claim(Some("a"), &wanted, true, &cancel);
        tokio::pin!(waiting);
        assert!(futures_util::poll!(waiting.as_mut()).is_pending());
        let circle = table
            .claim(Some("b"), &paths(&["src/a.rs"]), true, &cancel)
            .await;
        assert!(circle.unwrap_err().contains("waiting for files you hold"));
        table.release("b");
        assert!(waiting.await.is_ok());
        assert!(table.may_change(Some("a"), "src/b.rs").is_ok());
    }
}
