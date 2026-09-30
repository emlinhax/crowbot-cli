//! Every permission a call can ask for, and what granting it lets the call do. The tests hold the
//! secret rules and PLAN's locks to this list, so a new permission cannot slip past either.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    ReadsFiles,
    ChangesFiles,
    RunsCommands,
    Other,
}

pub struct Permission {
    pub name: &'static str,
    /// Read by the tests below, which hold the secret rules and PLAN's locks to it.
    #[cfg_attr(not(test), expect(dead_code))]
    pub effect: Effect,
}

const fn permission(name: &'static str, effect: Effect) -> Permission {
    Permission { name, effect }
}

pub const READ: Permission = permission("read", Effect::ReadsFiles);
/// glob and grep: they read what they search.
pub const SEARCH: Permission = permission("search", Effect::ReadsFiles);
/// write and edit.
pub const EDIT: Permission = permission("edit", Effect::ChangesFiles);
pub const BASH: Permission = permission("bash", Effect::RunsCommands);
/// A command line whose commands cannot be read off its text.
pub const BASH_COMPLEX: Permission = permission("bash_complex", Effect::RunsCommands);
/// A command line that redirects output into a file.
pub const BASH_WRITE: Permission = permission("bash_write", Effect::ChangesFiles);
/// Asked beside read, search or edit when the path leaves the project.
pub const EXTERNAL_DIRECTORY: Permission = permission("external_directory", Effect::Other);
pub const WEBFETCH: Permission = permission("webfetch", Effect::Other);
pub const CODESEARCH: Permission = permission("codesearch", Effect::Other);
pub const TODO: Permission = permission("todo", Effect::Other);
pub const QUESTION: Permission = permission("question", Effect::Other);
pub const PLAN_EXIT: Permission = permission("plan_exit", Effect::Other);
/// The same call repeated; asked by the agent, not a tool.
pub const DOOM_LOOP: Permission = permission("doom_loop", Effect::Other);

/// Every permission above; a new one goes here too, so the tests see it.
#[cfg(test)]
const ALL: &[Permission] = &[
    READ,
    SEARCH,
    EDIT,
    BASH,
    BASH_COMPLEX,
    BASH_WRITE,
    EXTERNAL_DIRECTORY,
    WEBFETCH,
    CODESEARCH,
    TODO,
    QUESTION,
    PLAN_EXIT,
    DOOM_LOOP,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode;
    use crate::permission::gate::{Ask, Decision, Policy};
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
        let rules = crate::settings::default_rules();
        for p in ALL.iter().filter(|p| p.effect == Effect::ReadsFiles) {
            for secret in [".env", "config/.env.local"] {
                let action = rule::evaluate(&rules, p.name, secret);
                assert_eq!(action, Action::Ask, "{} {secret}", p.name);
            }
        }
    }

    #[test]
    fn plan_denies_whatever_changes_files_or_runs_commands() {
        let rules = crate::settings::default_rules();
        let policy = Policy {
            rules: &rules,
            mode: mode::get("plan").unwrap(),
            plan_file: "/home/u/.crowbot/plans/s1.md",
        };
        for p in ALL
            .iter()
            .filter(|p| matches!(p.effect, Effect::ChangesFiles | Effect::RunsCommands))
        {
            let got = policy.decide(&[Ask::new(p.name, "make install")]);
            assert!(matches!(got, Decision::Deny(_)), "{}: {got:?}", p.name);
        }
    }
}
