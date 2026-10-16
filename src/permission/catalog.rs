//! Every permission a call can ask for, and what granting it allows. Tests hold the secret rules
//! and PLAN's locks to this list.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    ReadsFiles,
    ChangesFiles,
    RunsCommands,
    Other,
}

pub struct Permission {
    pub name: &'static str,
    pub effect: Effect,
}

/// Each permission is named once here: the macro makes its const and puts it in `ALL`, the list
/// the tests hold to the secret rules and PLAN's locks.
macro_rules! permissions {
    ($($(#[$doc:meta])* $name:ident = $id:literal, $effect:ident;)+) => {
        $(
            $(#[$doc])*
            pub const $name: Permission = Permission { name: $id, effect: Effect::$effect };
        )+

        const ALL: &[Permission] = &[$($name),+];
    };
}

/// Whether permission `name` lets a call change files or run commands, which may change them:
/// what the run snapshots the project before.
pub fn changes_things(name: &str) -> bool {
    ALL.iter()
        .find(|p| p.name == name)
        .is_some_and(|p| matches!(p.effect, Effect::ChangesFiles | Effect::RunsCommands))
}

permissions! {
    READ = "read", ReadsFiles;
    /// glob and grep: they read what they search.
    SEARCH = "search", ReadsFiles;
    /// write and edit.
    EDIT = "edit", ChangesFiles;
    BASH = "bash", RunsCommands;
    /// A command line whose commands cannot be read off its text.
    BASH_COMPLEX = "bash_complex", RunsCommands;
    /// A command line that redirects output into a file.
    BASH_WRITE = "bash_write", ChangesFiles;
    /// Asked beside read, search or edit when the path leaves the project.
    EXTERNAL_DIRECTORY = "external_directory", Other;
    WEBFETCH = "webfetch", Other;
    /// Reading a forum (sections, topics, threads, search); default-allowed, per host.
    FORUM = "forum", Other;
    /// Posting to a forum; the user confirms every post, whatever the rules allow.
    FORUM_POST = "forum_post", Other;
    TODO = "todo", Other;
    QUESTION = "question", Other;
    PLAN_EXIT = "plan_exit", Other;
    /// Starting a subagent, per kind; its own calls ask for themselves.
    TASK = "task", Other;
    /// The same call repeated; asked by the agent, not a tool.
    DOOM_LOOP = "doom_loop", Other;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode;
    use crate::permission::gate::{Ask, Decision};
    use crate::permission::rule::{self, Action};

    #[test]
    fn names_are_unique() {
        let mut names: Vec<_> = ALL.iter().map(|p| p.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn whatever_reads_files_asks_before_reading_a_secret() {
        let rules = crate::config::settings::default_rules();
        for p in ALL.iter().filter(|p| p.effect == Effect::ReadsFiles) {
            for secret in [".env", "config/.env.local"] {
                let action = rule::evaluate(&rules, p.name, secret);
                assert_eq!(action, Action::Ask, "{} {secret}", p.name);
            }
        }
    }

    #[test]
    fn plan_denies_whatever_changes_files_or_runs_commands() {
        let rules = crate::config::settings::default_rules();
        let plan = mode::get("plan").unwrap();
        let locks = plan.locks("/home/u/.crowbot/plans/s1.md");
        let policy = plan.policy(&rules, &locks);
        for p in ALL
            .iter()
            .filter(|p| matches!(p.effect, Effect::ChangesFiles | Effect::RunsCommands))
        {
            let got = policy.decide(&[Ask::new(p.name, "make install")]);
            assert!(matches!(got, Decision::Deny(_)), "{}: {got:?}", p.name);
        }
    }
}
