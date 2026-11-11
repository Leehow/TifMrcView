//! Where the Cryo-ET steps keep their files. The folder that holds the raw
//! tilt series (or the movie frames) is the project folder; every
//! intermediate step gets a subfolder of its own in it:
//!
//! ```text
//! <project>/            the raw tilt series / frames (never touched)
//!   1_motion/           motion-corrected tilt series made from frames
//!   2_tomograms/        <stem>.tomo.json, the tomograms and their halves
//!   3_clean/            what is made from a tomogram to look at: the look
//!                       companion, denoised, deconvolved, wedge-corrected
//!   4_sta/              subtomogram averaging
//! ```
//!
//! A project made before this layout has everything flat beside the tilt
//! series. It is recognised by its `<stem>.tomo.json` lying beside the stack
//! (`is_legacy`) and stays flat: every function here then answers with the
//! old flat place. Label maps stay beside the image they label and
//! thumbnails in `thumbnails/` beside the file; neither is decided here.

use std::path::{Path, PathBuf};

pub const MOTION: &str = "1_motion";
pub const TOMOGRAMS: &str = "2_tomograms";
pub const CLEAN: &str = "3_clean";
pub const STA: &str = "4_sta";
/// Every step folder, in the order of the steps.
pub const STEPS: [&str; 4] = [MOTION, TOMOGRAMS, CLEAN, STA];

/// Whether a folder name is one of the step folders.
pub fn is_step_folder(name: &str) -> bool {
    STEPS.contains(&name)
}

fn is_step_dir(dir: &Path) -> bool {
    dir.file_name().and_then(|n| n.to_str()).is_some_and(is_step_folder)
}

/// The folder of a file, `.` for a bare name.
fn folder_of(path: &Path) -> &Path {
    path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."))
}

/// The project folder of any file of a project: the file's folder, or that
/// folder's parent when the folder is one of the step folders.
pub fn root_of(path: &Path) -> PathBuf {
    let dir = folder_of(path);
    if is_step_dir(dir) {
        if let Some(up) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            return up.to_path_buf();
        }
    }
    dir.to_path_buf()
}

/// `root_of` for a recorded path written with either separator (a project
/// file may come from another system): the folder part, as a string.
pub fn root_str(path: &str) -> &str {
    let dir = crate::relocate::parent(path);
    let name = dir.rsplit(['/', '\\']).next().unwrap_or("");
    if is_step_folder(name) {
        let up = crate::relocate::parent(dir);
        if !up.is_empty() {
            return up;
        }
    }
    dir
}

/// An old flat project: the stack's dataset `<stem>.tomo.json` lies beside it.
pub fn is_legacy(stack: &str) -> bool {
    let p = Path::new(stack);
    p.with_file_name(format!("{}.tomo.json", stem_of(p))).is_file()
}

fn stem_of(p: &Path) -> String {
    p.file_stem().and_then(|s| s.to_str()).unwrap_or("stack").to_string()
}

/// The folder the files of `step` of the project of `stack` go to: the
/// step folder of the project, or, for an old flat project, the folder of
/// the stack. (Not created here.)
pub fn dir_for(stack: &str, step: &str) -> PathBuf {
    let p = Path::new(stack);
    if is_legacy(stack) {
        folder_of(p).to_path_buf()
    } else {
        root_of(p).join(step)
    }
}

/// Where the dataset json of a stack goes: `2_tomograms/<stem>.tomo.json`
/// of the project, or beside the stack for an old flat project.
pub fn dataset_path(stack: &str) -> PathBuf {
    dir_for(stack, TOMOGRAMS).join(format!("{}.tomo.json", stem_of(Path::new(stack))))
}

/// Where the output of kind `step` of `stack` (a name such as
/// `<stem>_sart3_b4.mrc`) goes; the folder is created when it is a step folder.
pub fn output_file(stack: &str, step: &str, name: &str) -> PathBuf {
    let dir = dir_for(stack, step);
    if is_step_dir(&dir) {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir.join(name)
}

/// Where a tilt series made from movie frames is written, given the place
/// it was asked for (beside the frames): `1_motion/` of the project. Left as
/// asked when it already lies in a step folder or beside a dataset (an old
/// flat project, which stays flat).
pub fn motion_output(output: &str) -> String {
    let p = Path::new(output);
    if is_legacy(output) || is_step_dir(folder_of(p)) {
        return output.to_string();
    }
    match p.file_name() {
        Some(name) => folder_of(p).join(MOTION).join(name).to_string_lossy().into_owned(),
        None => output.to_string(),
    }
}

/// The folder subtomogram averaging of `tomogram` works in: `4_sta/` of the
/// project when the tomogram lies in a step folder (`2_tomograms`,
/// `3_clean`), else `<stem>_sta` beside it (an old flat project, or a
/// tomogram from elsewhere).
pub fn sta_dir(tomogram: &str) -> PathBuf {
    let p = Path::new(tomogram);
    let dir = folder_of(p);
    if is_step_dir(dir) {
        return root_of(p).join(STA);
    }
    p.with_file_name(format!("{}_sta", stem_of(p)))
}

/// `dir` and the folders where files that belong with those of `dir` may be:
/// for a step folder the other step folders and the project folder (the
/// raw stacks), for any other folder just itself.
pub fn relatives(dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![dir.to_path_buf()];
    if is_step_dir(dir) {
        let root = root_of(&dir.join("x"));
        for s in STEPS.iter().map(|s| root.join(s)).chain([root.clone()]) {
            if !out.contains(&s) {
                out.push(s);
            }
        }
    }
    out
}

/// The file called `recorded`'s name in `dir` or the folders that go with it
/// (also when the project folder was copied: the recorded path is then
/// stale), else `recorded` itself when that is a file.
pub fn find_near(dir: &Path, recorded: &str) -> Option<PathBuf> {
    let name = Path::new(recorded).file_name()?;
    relatives(dir).into_iter().map(|d| d.join(name)).find(|f| f.is_file()).or_else(|| Path::new(recorded).is_file().then(|| PathBuf::from(recorded)))
}

/// The folders to look for datasets and tomograms in, given the folder of
/// the open file (or the project folder): in a new project the step
/// folders (and the project folder, last, for its raw stacks); a project
/// folder that has step folders in it: those, then itself; else (old flat layout) the
/// folder and the folders beside it, at most 200.
pub fn scan_folders(dir: &Path) -> Vec<PathBuf> {
    if is_step_dir(dir) {
        return relatives(dir).into_iter().filter(|d| d == dir || d.is_dir()).collect();
    }
    let inside: Vec<PathBuf> = STEPS.iter().map(|s| dir.join(s)).filter(|d| d.is_dir()).collect();
    if !inside.is_empty() {
        return inside.into_iter().chain(std::iter::once(dir.to_path_buf())).collect();
    }
    let mut out = vec![dir.to_path_buf()];
    if let Some(rd) = dir.parent().and_then(|up| std::fs::read_dir(up).ok()) {
        let mut beside: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.is_dir() && f != dir).collect();
        beside.sort();
        out.extend(beside.into_iter().take(200));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(p: &Path) -> String {
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn a_new_project_keeps_each_step_in_its_own_folder() {
        let dir = tempfile::tempdir().unwrap();
        let stack = s(&dir.path().join("ts_01.mrc"));
        std::fs::write(&stack, b"x").unwrap();
        assert!(!is_legacy(&stack));
        assert_eq!(root_of(Path::new(&stack)), dir.path());
        assert_eq!(dataset_path(&stack), dir.path().join("2_tomograms/ts_01.tomo.json"));
        assert_eq!(dir_for(&stack, CLEAN), dir.path().join("3_clean"));
        let f = output_file(&stack, CLEAN, "ts_01_look.mrc");
        assert_eq!(f, dir.path().join("3_clean/ts_01_look.mrc"));
        assert!(dir.path().join("3_clean").is_dir());
        assert!(!dir.path().join("4_sta").exists());
        assert_eq!(motion_output(&stack), s(&dir.path().join("1_motion/ts_01.mrc")));
    }

    #[test]
    fn an_old_flat_project_stays_flat() {
        let dir = tempfile::tempdir().unwrap();
        let stack = s(&dir.path().join("ts_01.mrc"));
        std::fs::write(dir.path().join("ts_01.tomo.json"), b"{}").unwrap();
        assert!(is_legacy(&stack));
        assert_eq!(dataset_path(&stack), dir.path().join("ts_01.tomo.json"));
        assert_eq!(output_file(&stack, TOMOGRAMS, "a.mrc"), dir.path().join("a.mrc"));
        assert_eq!(output_file(&stack, CLEAN, "b.mrc"), dir.path().join("b.mrc"));
        assert_eq!(motion_output(&stack), stack);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no folder was made");
    }

    #[test]
    fn a_stack_made_from_frames_lies_in_the_motion_folder_of_the_frames_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("1_motion")).unwrap();
        let stack = s(&dir.path().join("1_motion/ts_01.mrc"));
        assert_eq!(root_of(Path::new(&stack)), dir.path());
        assert_eq!(dataset_path(&stack), dir.path().join("2_tomograms/ts_01.tomo.json"));
        assert_eq!(dir_for(&stack, TOMOGRAMS), dir.path().join("2_tomograms"));
        assert_eq!(motion_output(&stack), stack);
        // The dataset is found as soon as it is saved there; the project is still not flat.
        std::fs::create_dir_all(dir.path().join("2_tomograms")).unwrap();
        std::fs::write(dataset_path(&stack), b"{}").unwrap();
        assert!(!is_legacy(&stack));
    }

    #[test]
    fn any_file_of_a_step_folder_belongs_to_the_project_folder() {
        let dir = tempfile::tempdir().unwrap();
        for step in STEPS {
            assert_eq!(root_of(&dir.path().join(step).join("x_look.mrc")), dir.path());
        }
        assert_eq!(root_of(&dir.path().join("sub/x.mrc")), dir.path().join("sub"));
        assert_eq!(root_str("/data/p/3_clean/x_look.mrc"), "/data/p");
        assert_eq!(root_str("C:\\data\\p\\1_motion\\x.mrc"), "C:\\data\\p");
        assert_eq!(root_str("/data/p/x.mrc"), "/data/p");
    }

    #[test]
    fn averaging_goes_to_the_sta_folder_of_a_new_project_only() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(sta_dir(&s(&dir.path().join("2_tomograms/t_sart3.mrc"))), dir.path().join("4_sta"));
        assert_eq!(sta_dir(&s(&dir.path().join("3_clean/t_look.mrc"))), dir.path().join("4_sta"));
        assert_eq!(sta_dir(&s(&dir.path().join("t_sart3.mrc"))), dir.path().join("t_sart3_sta"));
        assert_eq!(sta_dir("/data/a/t.mrc"), Path::new("/data/a/t_sta"));
    }

    #[test]
    fn files_of_a_project_are_found_in_the_folders_that_go_with_theirs() {
        let dir = tempfile::tempdir().unwrap();
        for step in [TOMOGRAMS, CLEAN] {
            std::fs::create_dir_all(dir.path().join(step)).unwrap();
        }
        std::fs::write(dir.path().join("3_clean/t_look.mrc"), b"x").unwrap();
        std::fs::write(dir.path().join("ts.mrc"), b"x").unwrap();
        let tomograms = dir.path().join("2_tomograms");
        assert_eq!(find_near(&tomograms, "/elsewhere/t_look.mrc"), Some(dir.path().join("3_clean/t_look.mrc")));
        assert_eq!(find_near(&tomograms, "/elsewhere/ts.mrc"), Some(dir.path().join("ts.mrc")));
        assert_eq!(find_near(&tomograms, "/elsewhere/none.mrc"), None);
        // A flat folder only looks in itself.
        assert_eq!(find_near(dir.path(), "/elsewhere/t_look.mrc"), None);
    }

    #[test]
    fn scanning_a_project_covers_its_step_folders_not_other_projects() {
        let up = tempfile::tempdir().unwrap();
        let (a, b) = (up.path().join("a"), up.path().join("b"));
        for d in [&a, &b] {
            std::fs::create_dir_all(d.join("2_tomograms")).unwrap();
            std::fs::create_dir_all(d.join("3_clean")).unwrap();
        }
        let from_root = scan_folders(&a);
        assert_eq!(from_root, [a.join("2_tomograms"), a.join("3_clean"), a.clone()]);
        let from_file = scan_folders(&a.join("3_clean"));
        assert_eq!(from_file, [a.join("3_clean"), a.join("2_tomograms"), a.clone()]);
        // Flat: the folder and the folders beside it.
        let flat = up.path().join("session/05");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::create_dir_all(up.path().join("session/06")).unwrap();
        assert_eq!(scan_folders(&flat), [flat.clone(), up.path().join("session/06")]);
    }
}
