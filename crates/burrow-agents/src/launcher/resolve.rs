//! Finding a program the way a Windows shell would, but only among files `CreateProcess` can run.
//!
//! npm installs three files for a CLI such as `claude`: a POSIX shell script without an
//! extension, `claude.ps1` and `claude.cmd`. Only the `.cmd` can be started from Rust. So a bare
//! name is looked up in each `PATH` directory with the extensions in `PATHEXT`, in order, keeping
//! only `.exe`, `.com`, `.bat` and `.cmd`.

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Extensions `CreateProcess` can start, directly or through `cmd.exe`.
const RUNNABLE: &[&str] = &[".exe", ".com", ".bat", ".cmd"];
/// `PATHEXT` when it isn't set.
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// Finds `name` on `PATH`. See [`find_program_in`].
pub fn find_program(name: &str) -> Option<PathBuf> {
    find_program_in(name, &[])
}

/// Finds `name` the way the shell would, then in `extra_dirs`.
///
/// - A name with a directory part (`C:\tools\x.exe`, `.\x`) is only checked where it points.
/// - A bare name is looked up in every `PATH` directory, then in `extra_dirs`: install locations
///   to try when `burrowd`'s `PATH` is older than the install.
/// - Without an extension, the `PATHEXT` extensions are tried in order. Only `.exe`, `.com`,
///   `.bat` and `.cmd` count; `.ps1` and extensionless shell scripts can't be started directly.
pub fn find_program_in(name: &str, extra_dirs: &[PathBuf]) -> Option<PathBuf> {
    let path = Path::new(name);
    let extensions = extensions();
    if path.components().count() > 1 || path.is_absolute() {
        return candidates(path, &extensions).find(|candidate| candidate.is_file());
    }
    let dirs = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    dirs.iter()
        .chain(extra_dirs)
        .filter(|dir| !dir.as_os_str().is_empty())
        .flat_map(|dir| candidates(&dir.join(name), &extensions).collect::<Vec<_>>())
        .find(|candidate| candidate.is_file())
}

/// The runnable extensions from `PATHEXT`, lower case, in order.
fn extensions() -> Vec<String> {
    let pathext = env::var("PATHEXT").unwrap_or_else(|_| DEFAULT_PATHEXT.to_owned());
    let mut extensions: Vec<String> = pathext
        .split(';')
        .map(|ext| ext.trim().to_ascii_lowercase())
        .filter(|ext| RUNNABLE.contains(&ext.as_str()))
        .collect();
    if extensions.is_empty() {
        extensions = RUNNABLE.iter().map(|ext| (*ext).to_owned()).collect();
    }
    extensions
}

/// The files `base` may mean: itself if it already has a runnable extension, else `base` plus
/// each extension.
fn candidates<'a>(base: &'a Path, extensions: &'a [String]) -> impl Iterator<Item = PathBuf> + 'a {
    let has_runnable_extension = base
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| RUNNABLE.contains(&format!(".{}", ext.to_ascii_lowercase()).as_str()));
    let own = has_runnable_extension.then(|| base.to_path_buf());
    let with_extensions = extensions
        .iter()
        .filter(move |_| !has_runnable_extension)
        .map(|ext| {
            let mut name = OsString::from(base.as_os_str());
            name.push(ext);
            PathBuf::from(name)
        });
    own.into_iter().chain(with_extensions)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_cmd_shim_wins_over_ps1_and_extensionless_scripts() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "burrow-shim-test",
            "burrow-shim-test.ps1",
            "burrow-shim-test.cmd",
        ] {
            fs::write(dir.path().join(file), "").unwrap();
        }
        let found = find_program_in("burrow-shim-test", &[dir.path().to_path_buf()]).unwrap();
        assert_eq!(found, dir.path().join("burrow-shim-test.cmd"));
    }

    #[test]
    fn exe_comes_before_cmd_and_explicit_paths_are_checked_where_they_point() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("burrow-tool-test.cmd"), "").unwrap();
        fs::write(dir.path().join("burrow-tool-test.exe"), "").unwrap();
        assert_eq!(
            find_program_in("burrow-tool-test", &[dir.path().to_path_buf()]).unwrap(),
            dir.path().join("burrow-tool-test.exe")
        );
        let explicit = dir.path().join("burrow-tool-test.cmd");
        assert_eq!(
            find_program_in(explicit.to_str().unwrap(), &[]).unwrap(),
            explicit
        );
        let without_extension = dir.path().join("burrow-tool-test");
        assert_eq!(
            find_program_in(without_extension.to_str().unwrap(), &[]).unwrap(),
            dir.path().join("burrow-tool-test.exe")
        );
        assert!(find_program_in("no-such-tool-anywhere", &[dir.path().to_path_buf()]).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn finds_cmd_exe_on_path() {
        let found = find_program("cmd").unwrap();
        assert!(
            found
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with("cmd.exe"),
            "{}",
            found.display()
        );
    }
}
