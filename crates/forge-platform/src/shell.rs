//! Which shell runs commands (Bash tool, `!` commands, hooks, the status line,
//! `apiKeyHelper`, custom-command expansions, forge-eval checks) and how a
//! script is handed to it.
//!
//! Order:
//! 1. `FORGE_SHELL` (process environment, else the settings `env` block):
//!    a path, or a name looked up on PATH. A wrong value is an error, never
//!    silently skipped.
//! 2. Windows: Git Bash (next to `git.exe` on PATH, then the standard install
//!    directories, then a `bash.exe` on PATH that isn't the WSL launcher);
//!    then PowerShell 7 (`pwsh.exe`), then Windows PowerShell 5.1.
//!    Unix: bash (fixed paths, then PATH), then sh.
//! 3. Otherwise [`ShellMissing`], whose message names every place looked in and the fix.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Unix,
    Windows,
}

impl Os {
    pub const CURRENT: Os = if cfg!(windows) { Os::Windows } else { Os::Unix };

    fn sep(self) -> char {
        match self {
            Os::Unix => '/',
            Os::Windows => '\\',
        }
    }

    fn list_sep(self) -> char {
        match self {
            Os::Unix => ':',
            Os::Windows => ';',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// bash, including Git Bash, MSYS2 and Cygwin bash on Windows.
    Bash,
    /// Another POSIX shell: sh, dash, ash, ksh, zsh.
    Posix,
    /// `pwsh` (PowerShell 7+) or `powershell.exe` (Windows PowerShell 5.1).
    PowerShell,
}

/// Where the shell came from (shown by `forge doctor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// `FORGE_SHELL` in the process environment.
    EnvVar,
    /// `FORGE_SHELL` in a settings file's `env` block.
    SettingsEnv,
    /// Git for Windows, located from `git.exe` on PATH.
    NextToGit,
    /// A standard install directory.
    Standard,
    /// A bare name on PATH.
    Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shell {
    pub kind: ShellKind,
    pub program: PathBuf,
    pub found: Found,
    pub os: Os,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BadOverride {
    /// The path (or name) doesn't exist.
    Missing,
    /// It exists but isn't a shell Forge can drive (cmd, fish, nu, ...).
    Unsupported,
}

/// No usable shell. `to_string()` is the exact text shown to the user and the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellMissing {
    pub os: Os,
    /// The bash / POSIX candidates checked, in order, as the message names them.
    pub looked_for: Vec<String>,
    /// The PowerShell candidates checked (Windows, unless only POSIX shells qualify).
    pub powershell: Vec<String>,
    /// `FORGE_SHELL` was set but unusable: its value, where it was set, why.
    pub bad_override: Option<(String, &'static str, BadOverride)>,
}

pub type ShellChoice = Result<Shell, ShellMissing>;

#[derive(Debug, Clone, Default)]
pub struct ShellConfig {
    /// `FORGE_SHELL` from the process environment.
    pub env_override: Option<String>,
    /// `FORGE_SHELL` from the settings `env` block (used when the process has none).
    pub settings_override: Option<String>,
    /// Only bash / POSIX shells qualify (forge-eval checks, `.sh` scripts).
    pub posix_only: bool,
}

impl ShellConfig {
    /// From the process environment only.
    pub fn from_env() -> Self {
        ShellConfig {
            env_override: std::env::var("FORGE_SHELL").ok().filter(|v| !v.trim().is_empty()),
            ..Default::default()
        }
    }
}

/// What resolution may look at; the real one reads the environment and the
/// filesystem, tests pass fakes (so Windows rules run on Linux CI).
pub struct Probe<'a> {
    pub os: Os,
    pub var: &'a dyn Fn(&str) -> Option<String>,
    pub is_file: &'a dyn Fn(&str) -> bool,
}

pub fn resolve(cfg: &ShellConfig) -> ShellChoice {
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let is_file = |p: &str| Path::new(p).is_file();
    resolve_with(cfg, &Probe { os: Os::CURRENT, var: &var, is_file: &is_file })
}

/// The shell for code with no session (tests, `forge mcp serve`): process environment only, probed once.
pub fn detect() -> &'static ShellChoice {
    static S: OnceLock<ShellChoice> = OnceLock::new();
    S.get_or_init(|| resolve(&ShellConfig::from_env()))
}

fn join(os: Os, base: &str, rest: &str) -> String {
    let sep = os.sep();
    let rest = if os == Os::Windows { rest.replace('/', "\\") } else { rest.to_string() };
    format!("{}{sep}{}", base.trim_end_matches(['\\', '/']), rest)
}

fn path_dirs(os: Os, p: &Probe) -> Vec<String> {
    (p.var)("PATH")
        .unwrap_or_default()
        .split(os.list_sep())
        .map(|d| d.trim().trim_matches('"').to_string())
        .filter(|d| !d.is_empty())
        .collect()
}

fn exe(os: Os, name: &str) -> String {
    if os == Os::Windows && !name.to_ascii_lowercase().ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `name` on PATH, skipping directories `skip` rejects.
fn which(os: Os, p: &Probe, name: &str, skip: &dyn Fn(&str) -> bool) -> Option<String> {
    let file = exe(os, name);
    path_dirs(os, p).into_iter().filter(|d| !skip(d)).map(|d| join(os, &d, &file)).find(|c| (p.is_file)(c))
}

fn file_stem(os: Os, program: &str) -> String {
    let name = program.rsplit(|c| c == '/' || (os == Os::Windows && c == '\\')).next().unwrap_or(program);
    let lower = name.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

pub fn kind_of(os: Os, program: &str) -> Option<ShellKind> {
    match file_stem(os, program).as_str() {
        "bash" => Some(ShellKind::Bash),
        "sh" | "dash" | "ash" | "ksh" | "mksh" | "zsh" => Some(ShellKind::Posix),
        "pwsh" | "powershell" => Some(ShellKind::PowerShell),
        _ => None,
    }
}

fn is_wsl_launcher(os: Os, p: &Probe, dir: &str) -> bool {
    let d = dir.to_ascii_lowercase();
    let sysroot = (p.var)("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()).to_ascii_lowercase();
    os == Os::Windows && (d.starts_with(&sysroot) || d.contains(r"\windowsapps"))
}

/// Git for Windows roots: from `git.exe` on PATH (`<root>\cmd`, `<root>\bin`,
/// `<root>\mingw64\bin`), then the standard install directories.
fn git_roots(p: &Probe) -> Vec<(String, Found)> {
    let os = Os::Windows;
    let mut out = vec![];
    for d in path_dirs(os, p) {
        if !(p.is_file)(&join(os, &d, "git.exe")) {
            continue;
        }
        let lower = d.to_ascii_lowercase();
        let lower = lower.trim_end_matches('\\');
        let up = |n: usize| -> String {
            let mut s = d.trim_end_matches('\\').to_string();
            for _ in 0..n {
                if let Some(i) = s.rfind('\\') {
                    s.truncate(i);
                }
            }
            s
        };
        if lower.ends_with(r"\mingw64\bin") || lower.ends_with(r"\usr\bin") {
            out.push((up(2), Found::NextToGit));
        } else if lower.ends_with(r"\cmd") || lower.ends_with(r"\bin") {
            out.push((up(1), Found::NextToGit));
        }
    }
    for (var, rest) in [
        ("ProgramFiles", "Git"),
        ("ProgramW6432", "Git"),
        ("ProgramFiles(x86)", "Git"),
        ("LOCALAPPDATA", r"Programs\Git"),
        ("USERPROFILE", r"scoop\apps\git\current"),
    ] {
        if let Some(base) = (p.var)(var) {
            out.push((join(os, &base, rest), Found::Standard));
        }
    }
    let mut seen = vec![];
    out.retain(|(r, _)| {
        let k = r.to_ascii_lowercase();
        !seen.contains(&k) && {
            seen.push(k);
            true
        }
    });
    out
}

pub fn resolve_with(cfg: &ShellConfig, p: &Probe) -> ShellChoice {
    let os = p.os;
    let mut looked_for: Vec<String> = vec![];
    let mut powershell: Vec<String> = vec![];

    // 1. FORGE_SHELL.
    let over = match (&cfg.env_override, &cfg.settings_override) {
        (Some(v), _) => Some((v.trim().to_string(), Found::EnvVar)),
        (None, Some(v)) => Some((v.trim().to_string(), Found::SettingsEnv)),
        _ => None,
    };
    if let Some((value, found)) = over {
        let origin = if found == Found::EnvVar { "the environment" } else { "the settings env block" };
        let is_path = value.contains('/') || (os == Os::Windows && (value.contains('\\') || value.contains(':')));
        let program =
            if is_path { (p.is_file)(&value).then(|| value.clone()) } else { which(os, p, &value, &|_| false) };
        let kind = kind_of(os, &value);
        let skip = cfg.posix_only && kind == Some(ShellKind::PowerShell);
        if !skip {
            return match (program, kind) {
                (Some(program), Some(kind)) => Ok(Shell { kind, program: PathBuf::from(program), found, os }),
                (None, _) => Err(ShellMissing {
                    os,
                    looked_for: vec![value.clone()],
                    powershell: vec![],
                    bad_override: Some((value, origin, BadOverride::Missing)),
                }),
                (Some(_), None) => Err(ShellMissing {
                    os,
                    looked_for: vec![value.clone()],
                    powershell: vec![],
                    bad_override: Some((value, origin, BadOverride::Unsupported)),
                }),
            };
        }
    }

    let ok = |kind, program: String, found| Ok(Shell { kind, program: PathBuf::from(program), found, os });
    match os {
        Os::Windows => {
            // 2. Git Bash.
            for (root, found) in git_roots(p) {
                for rel in [r"bin\bash.exe", r"usr\bin\bash.exe"] {
                    let c = join(os, &root, rel);
                    if (p.is_file)(&c) {
                        return ok(ShellKind::Bash, c, found);
                    }
                }
                looked_for.push(join(os, &root, r"bin\bash.exe"));
            }
            if let Some(c) = which(os, p, "bash", &|d| is_wsl_launcher(os, p, d)) {
                return ok(ShellKind::Bash, c, Found::Path);
            }
            looked_for.push("bash.exe on PATH".into());
            if cfg.posix_only {
                return Err(ShellMissing { os, looked_for, powershell, bad_override: None });
            }
            // 3. PowerShell.
            if let Some(c) = which(os, p, "pwsh", &|_| false) {
                return ok(ShellKind::PowerShell, c, Found::Path);
            }
            if let Some(base) = (p.var)("ProgramFiles") {
                let c = join(os, &base, r"PowerShell\7\pwsh.exe");
                if (p.is_file)(&c) {
                    return ok(ShellKind::PowerShell, c, Found::Standard);
                }
            }
            powershell.push("pwsh.exe on PATH".into());
            let sysroot = (p.var)("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
            let c = join(os, &sysroot, r"System32\WindowsPowerShell\v1.0\powershell.exe");
            if (p.is_file)(&c) {
                return ok(ShellKind::PowerShell, c, Found::Standard);
            }
            powershell.push(c);
        }
        Os::Unix => {
            for c in ["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash", "/opt/homebrew/bin/bash"] {
                if (p.is_file)(c) {
                    return ok(ShellKind::Bash, c.into(), Found::Standard);
                }
                looked_for.push(c.into());
            }
            if let Some(c) = which(os, p, "bash", &|_| false) {
                return ok(ShellKind::Bash, c, Found::Path);
            }
            looked_for.push("bash on PATH".into());
            if (p.is_file)("/bin/sh") {
                return ok(ShellKind::Posix, "/bin/sh".into(), Found::Standard);
            }
            looked_for.push("/bin/sh".into());
            if let Some(c) = which(os, p, "sh", &|_| false) {
                return ok(ShellKind::Posix, c, Found::Path);
            }
            looked_for.push("sh on PATH".into());
        }
    }
    Err(ShellMissing { os, looked_for, powershell, bad_override: None })
}

/// The `Shell:` line of the model's environment prompt: nothing for bash on Unix
/// (the usual case, so that prompt is unchanged), the shell otherwise.
pub fn env_line(choice: &ShellChoice) -> Option<String> {
    match choice {
        Ok(s) if s.os == Os::Unix && s.kind == ShellKind::Bash => None,
        Ok(s) => Some(s.describe()),
        Err(_) => Some("none found; the Bash tool can't run commands in this session".into()),
    }
}

/// "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

impl fmt::Display for ShellMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((value, origin, why)) = &self.bad_override {
            let example = match self.os {
                Os::Windows => r"C:\Program Files\Git\bin\bash.exe",
                Os::Unix => "/bin/bash",
            };
            return match why {
                BadOverride::Missing => write!(
                    f,
                    "FORGE_SHELL is set to {value} (in {origin}), but no such program exists. Set it to the full \
                     path of a shell, for example {example}, or remove it so Forge finds one itself."
                ),
                BadOverride::Unsupported => write!(
                    f,
                    "FORGE_SHELL is set to {value} (in {origin}), which Forge can't run commands with. Forge \
                     supports bash, sh and zsh, and PowerShell (pwsh or powershell). Set FORGE_SHELL to one of \
                     those, for example {example}, or remove it."
                ),
            };
        }
        match self.os {
            Os::Windows if self.powershell.is_empty() => write!(
                f,
                "No shell found to run commands. Forge looked for Git Bash in {}. Install Git for Windows \
                 (https://git-scm.com/downloads/win), or set FORGE_SHELL to the full path of bash.exe, then \
                 restart Forge.",
                and_list(&self.looked_for)
            ),
            Os::Windows => write!(
                f,
                "No shell found to run commands. Forge looked for Git Bash in {}, and for PowerShell ({}). \
                 Install Git for Windows (https://git-scm.com/downloads/win), or set FORGE_SHELL to the full \
                 path of bash.exe or pwsh.exe, then restart Forge.",
                and_list(&self.looked_for),
                self.powershell.join(", ")
            ),
            Os::Unix => write!(
                f,
                "No shell found to run commands. Forge looked for {}. Install bash, or set FORGE_SHELL to the \
                 full path of a POSIX shell, then restart Forge.",
                and_list(&self.looked_for)
            ),
        }
    }
}

impl std::error::Error for ShellMissing {}

impl Shell {
    pub fn is_posix(&self) -> bool {
        self.kind != ShellKind::PowerShell
    }

    /// Short name: "Git Bash", "bash", "sh", "PowerShell 7", "Windows PowerShell 5.1".
    pub fn label(&self) -> String {
        let stem = file_stem(self.os, &self.program.to_string_lossy());
        match (self.kind, self.os) {
            (ShellKind::Bash, Os::Windows) => {
                let p = self.program.to_string_lossy().to_ascii_lowercase();
                if p.contains(r"\git\") {
                    "Git Bash".into()
                } else {
                    "bash".into()
                }
            }
            (ShellKind::PowerShell, _) if stem == "pwsh" => "PowerShell 7".into(),
            (ShellKind::PowerShell, _) => "Windows PowerShell 5.1".into(),
            _ => stem,
        }
    }

    /// "Git Bash (C:\Program Files\Git\bin\bash.exe)".
    pub fn describe(&self) -> String {
        format!("{} ({})", self.label(), self.program.display())
    }

    /// The script run for `command`: the command, then (with `cwd_file`) its
    /// final directory written to that file, keeping the command's exit status.
    pub fn script(&self, command: &str, cwd_file: Option<&Path>) -> String {
        let Some(f) = cwd_file else {
            return match self.kind {
                ShellKind::PowerShell => format!("{}{command}\n{}", ps_prelude(), PS_EXIT),
                _ => command.to_string(),
            };
        };
        let file = f.display().to_string();
        match self.kind {
            ShellKind::PowerShell => format!(
                "{}{command}\n$__forge_ok = $?; $__forge_code = $LASTEXITCODE\n\
                 try {{ [IO.File]::WriteAllText('{}', (Get-Location).ProviderPath) }} catch {{ }}\n{}",
                ps_prelude(),
                file.replace('\'', "''"),
                PS_EXIT_SAVED
            ),
            _ => {
                let file = if self.os == Os::Windows { file.replace('\\', "/") } else { file };
                format!(
                    "{command}\n__forge_ec=$?\npwd -P > '{}' 2>/dev/null\nexit $__forge_ec",
                    file.replace('\'', "'\\''")
                )
            }
        }
    }

    /// The directory a command finished in, from what [`Shell::script`] wrote.
    pub fn parse_cwd(&self, written: &str) -> Option<PathBuf> {
        let s = written.trim_start_matches('\u{feff}').trim();
        if s.is_empty() {
            return None;
        }
        if self.os == Os::Windows && self.is_posix() {
            return crate::path::msys_to_windows(s).map(PathBuf::from);
        }
        Some(PathBuf::from(s))
    }

    /// A ready command that runs `script`. Unix: `<shell> -c <script>` (pwsh:
    /// `-Command`). Windows: the script goes to a temporary file, so no
    /// command-line quoting (MSYS and PowerShell each parse it their own way)
    /// or length limit applies; keep the returned [`ScriptFile`] until the process exits.
    pub fn command(&self, script: &str) -> std::io::Result<(std::process::Command, Option<ScriptFile>)> {
        let mut c = std::process::Command::new(&self.program);
        if self.os == Os::Unix || cfg!(not(windows)) {
            match self.kind {
                ShellKind::PowerShell => c.args(["-NoProfile", "-NonInteractive", "-Command", script]),
                _ => c.arg("-c").arg(script),
            };
            return Ok((c, None));
        }
        let file = ScriptFile::write(script, self.kind)?;
        let path = file.0.display().to_string();
        match self.kind {
            ShellKind::PowerShell => {
                c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"]).arg(&path);
            }
            _ => {
                c.arg(path.replace('\\', "/"));
            }
        }
        Ok((c, Some(file)))
    }
}

fn ps_prelude() -> &'static str {
    "$ProgressPreference = 'SilentlyContinue'\n\
     $PSDefaultParameterValues['*:Encoding'] = 'utf8'\n\
     try { [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false) } catch { }\n\
     $OutputEncoding = [Text.UTF8Encoding]::new($false)\n"
}

const PS_EXIT: &str = "if ($?) { exit 0 } elseif ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 }";
const PS_EXIT_SAVED: &str = "if ($__forge_ok) { exit 0 } elseif ($__forge_code) { exit $__forge_code } else { exit 1 }";

/// A temporary script file, removed when dropped.
#[derive(Debug)]
pub struct ScriptFile(pub PathBuf);

impl ScriptFile {
    fn write(script: &str, kind: ShellKind) -> std::io::Result<ScriptFile> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let ext = if kind == ShellKind::PowerShell { "ps1" } else { "sh" };
        let path = std::env::temp_dir().join(format!(
            "forge-cmd-{}-{}.{ext}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
        if kind == ShellKind::PowerShell {
            // Windows PowerShell 5.1 reads a BOM-less script in the ANSI code page.
            f.write_all(b"\xEF\xBB\xBF")?;
        }
        f.write_all(script.as_bytes())?;
        Ok(ScriptFile(path))
    }
}

impl Drop for ScriptFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn probe_run(os: Os, vars: &[(&str, &str)], files: &[&str], cfg: &ShellConfig) -> ShellChoice {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let files: HashSet<String> = files.iter().map(|s| s.to_string()).collect();
        let var = |k: &str| vars.get(k).cloned();
        let is_file = |p: &str| files.contains(p);
        resolve_with(cfg, &Probe { os, var: &var, is_file: &is_file })
    }

    const WIN_VARS: &[(&str, &str)] = &[
        ("ProgramFiles", r"C:\Program Files"),
        ("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        ("LOCALAPPDATA", r"C:\Users\me\AppData\Local"),
        ("SystemRoot", r"C:\Windows"),
    ];

    #[test]
    fn windows_prefers_git_bash_next_to_git_on_path() {
        let mut vars = WIN_VARS.to_vec();
        vars.push(("PATH", r"C:\Windows\System32;D:\Tools\Git\cmd;C:\Windows\System32\WindowsPowerShell\v1.0"));
        let files = [
            r"D:\Tools\Git\cmd\git.exe",
            r"D:\Tools\Git\bin\bash.exe",
            r"C:\Windows\System32\bash.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ];
        let s = probe_run(Os::Windows, &vars, &files, &ShellConfig::default()).unwrap();
        assert_eq!(s.program, PathBuf::from(r"D:\Tools\Git\bin\bash.exe"));
        assert_eq!((s.kind, s.found), (ShellKind::Bash, Found::NextToGit));
        assert_eq!(s.label(), "Git Bash");
    }

    #[test]
    fn windows_finds_standard_git_install_and_skips_wsl_bash() {
        let mut vars = WIN_VARS.to_vec();
        vars.push(("PATH", r"C:\Windows\System32"));
        let files = [r"C:\Windows\System32\bash.exe", r"C:\Users\me\AppData\Local\Programs\Git\bin\bash.exe"];
        let s = probe_run(Os::Windows, &vars, &files, &ShellConfig::default()).unwrap();
        assert_eq!(s.program, PathBuf::from(r"C:\Users\me\AppData\Local\Programs\Git\bin\bash.exe"));
    }

    #[test]
    fn windows_falls_back_to_powershell_but_not_for_posix_only() {
        let mut vars = WIN_VARS.to_vec();
        vars.push(("PATH", r"C:\Windows\System32"));
        let files = [r"C:\Windows\System32\bash.exe", r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"];
        let s = probe_run(Os::Windows, &vars, &files, &ShellConfig::default()).unwrap();
        assert_eq!(s.kind, ShellKind::PowerShell);
        assert_eq!(s.label(), "Windows PowerShell 5.1");
        let cfg = ShellConfig { posix_only: true, ..Default::default() };
        let e = probe_run(Os::Windows, &vars, &files, &cfg).unwrap_err();
        let msg = e.to_string();
        assert!(msg.contains(r"C:\Program Files\Git\bin\bash.exe"), "{msg}");
        assert!(msg.contains("Install Git for Windows"), "{msg}");
        assert!(msg.contains("FORGE_SHELL"), "{msg}");
    }

    #[test]
    fn override_wins_and_a_bad_one_is_an_error() {
        let cfg = ShellConfig { env_override: Some(r"C:\Git\bin\bash.exe".into()), ..Default::default() };
        let e = probe_run(Os::Windows, WIN_VARS, &[r"C:\Program Files\Git\bin\bash.exe"], &cfg).unwrap_err();
        assert_eq!(
            e.to_string(),
            r"FORGE_SHELL is set to C:\Git\bin\bash.exe (in the environment), but no such program exists. Set it to the full path of a shell, for example C:\Program Files\Git\bin\bash.exe, or remove it so Forge finds one itself."
        );
        let cfg = ShellConfig { settings_override: Some("cmd.exe".into()), ..Default::default() };
        let mut vars = WIN_VARS.to_vec();
        vars.push(("PATH", r"C:\Windows\System32"));
        let e = probe_run(Os::Windows, &vars, &[r"C:\Windows\System32\cmd.exe"], &cfg).unwrap_err();
        assert!(e.to_string().contains("can't run commands with"), "{e}");
        let cfg = ShellConfig { env_override: Some("pwsh".into()), ..Default::default() };
        let mut vars = WIN_VARS.to_vec();
        vars.push(("PATH", r"C:\Program Files\PowerShell\7"));
        let s = probe_run(Os::Windows, &vars, &[r"C:\Program Files\PowerShell\7\pwsh.exe"], &cfg).unwrap();
        assert_eq!((s.kind, s.found), (ShellKind::PowerShell, Found::EnvVar));
    }

    #[test]
    fn unix_order_and_message() {
        let s = probe_run(Os::Unix, &[("PATH", "/usr/bin")], &["/bin/sh"], &ShellConfig::default()).unwrap();
        assert_eq!((s.kind, s.program.clone()), (ShellKind::Posix, PathBuf::from("/bin/sh")));
        let s = probe_run(
            Os::Unix,
            &[("PATH", "/run/current-system/sw/bin")],
            &["/run/current-system/sw/bin/bash", "/bin/sh"],
            &ShellConfig::default(),
        )
        .unwrap();
        assert_eq!(s.program, PathBuf::from("/run/current-system/sw/bin/bash"));
        let e = probe_run(Os::Unix, &[("PATH", "/usr/bin")], &[], &ShellConfig::default()).unwrap_err();
        assert_eq!(
            e.to_string(),
            "No shell found to run commands. Forge looked for /bin/bash, /usr/bin/bash, /usr/local/bin/bash, \
             /opt/homebrew/bin/bash, bash on PATH, /bin/sh and sh on PATH. Install bash, or set FORGE_SHELL to \
             the full path of a POSIX shell, then restart Forge."
        );
    }

    #[test]
    fn scripts_and_cwd() {
        let gb = Shell {
            kind: ShellKind::Bash,
            program: r"C:\Program Files\Git\bin\bash.exe".into(),
            found: Found::Standard,
            os: Os::Windows,
        };
        let s = gb.script("echo hi", Some(Path::new(r"C:\Users\me\AppData\Local\Temp\forge-cwd-1")));
        assert!(s.contains("pwd -P > 'C:/Users/me/AppData/Local/Temp/forge-cwd-1'"), "{s}");
        assert_eq!(gb.parse_cwd("/c/Users/me/proj\n"), Some(PathBuf::from(r"C:\Users\me\proj")));
        assert_eq!(gb.parse_cwd("/usr/bin"), None);
        let ps = Shell { kind: ShellKind::PowerShell, ..gb.clone() };
        let s = ps.script("Get-ChildItem", Some(Path::new(r"C:\T\it's")));
        assert!(s.contains(r"WriteAllText('C:\T\it''s'"), "{s}");
        assert_eq!(ps.parse_cwd("\u{feff}C:\\proj\r\n"), Some(PathBuf::from("C:\\proj")));
    }
}
