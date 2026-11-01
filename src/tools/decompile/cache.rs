//! kuna's answers kept on disk. kuna loads and analyses the whole binary on every run, so the same
//! question about the same files is put to it once.

use std::path::{Path, PathBuf};

use crate::config::paths::Paths;
use crate::io::fs::{self, Access};
use crate::text::digest;

/// Where the answer to `argv` is kept. Named for kuna, the binary and a PDB beside it as they are
/// now (path, size, modified time), so a rebuild of any of them, or a new kuna, asks again.
// CEILING: a PDB found elsewhere (the path the binary names, a symbol server) is not in the name,
// so a change to it alone is not seen. Upgrade: read the binary's CodeView record for its PDB.
// CEILING: nothing is ever removed; an answer is a few KB of text. Upgrade: drop old files on put.
pub fn file(paths: &Paths, kuna: &Path, binary: &Path, argv: &[String]) -> PathBuf {
    let pdb = binary.with_extension("pdb");
    let mut what = String::new();
    for file in [kuna, binary, &pdb] {
        let (size, modified) = (fs::size(file), fs::modified(file));
        what += &format!("{}\0{size:?}\0{modified:?}\n", file.display());
    }
    what += &argv.join("\0");
    let name = &digest::sha256_hex(what.as_bytes())[..32];
    paths.decompile_cache().join(format!("{name}.txt"))
}

pub fn get(file: &Path) -> Option<String> {
    fs::read_string(file).ok().flatten()
}

/// Best effort: an answer that cannot be kept is only asked for again.
pub fn put(file: &Path, answer: &str) {
    let _ = fs::write_atomic(file, answer.as_bytes(), Access::Private);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testing::Project;

    #[test]
    fn the_same_files_and_question_find_the_same_answer() {
        let project = Project::new();
        let paths = &project.app.paths;
        let (kuna, binary) = (paths.project.join("kuna"), paths.project.join("prog.exe"));
        project.write("kuna", "kuna 1");
        project.write("prog.exe", "MZ one");
        let ask = |what: &str| file(paths, &kuna, &binary, &[what.to_owned()]);

        let first = ask("functions");
        assert_eq!(get(&first), None);
        put(&first, "main\n");
        assert_eq!(get(&ask("functions")).as_deref(), Some("main\n"));
        assert_ne!(ask("decompile main"), first);

        project.write("prog.pdb", "symbols");
        let with_pdb = ask("functions");
        assert_ne!(with_pdb, first);
        project.write("prog.exe", "MZ rebuilt");
        assert_ne!(ask("functions"), with_pdb);
        project.write("kuna", "kuna 2, newer");
        assert_ne!(ask("functions"), with_pdb);
    }
}
