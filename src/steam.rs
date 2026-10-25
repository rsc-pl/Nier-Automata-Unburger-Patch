use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn installations() -> Vec<PathBuf> {
    let mut bases = BTreeSet::<PathBuf>::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        for dir in [".steam/steam", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"] {
            bases.insert(home.join(dir));
        }
    }
    for key in ["PROGRAMFILES(X86)", "PROGRAMFILES", "ProgramFiles", "ProgramFiles(x86)"] {
        if let Ok(base) = std::env::var(key) { bases.insert(PathBuf::from(base).join("Steam")); }
    }
    if let Ok(path) = std::env::var("STEAM_DIR") { bases.insert(PathBuf::from(path)); }
    #[cfg(windows)]
    if let Ok(result) = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
    {
        if result.status.success() {
            for line in String::from_utf8_lossy(&result.stdout).lines() {
                if let Some(path) = line.split("REG_SZ").nth(1) {
                    let value = path.trim();
                    if !value.is_empty() { bases.insert(PathBuf::from(value)); }
                }
            }
        }
    }
    let mut libraries = BTreeSet::<PathBuf>::new();
    for base in bases {
        let direct = base.join("steamapps");
        if direct.is_dir() { libraries.insert(direct.clone()); }
        let listing = direct.join("libraryfolders.vdf");
        if let Ok(text) = fs::read_to_string(listing) {
            for line in text.lines() {
                if let Some(path) = vdf_path(line) {
                    let p = PathBuf::from(path).join("steamapps");
                    if p.is_dir() { libraries.insert(p); }
                }
            }
        }
    }
    let mut found = BTreeSet::<PathBuf>::new();
    for dir in libraries {
        let install_dir = dir.join("common");
        for title in ["NieRAutomata", "NieR Automata", "NieR:Automata", "NieR_Automata"] {
            let candidate = install_dir.join(title);
            if valid_installation(&candidate) { found.insert(candidate); }
        }
        // Respect Steam's actual install directory in appmanifest_524220.acf.
        if let Ok(text) = fs::read_to_string(dir.join("appmanifest_524220.acf")) {
            for line in text.lines() {
                if let Some(name) = vdf_value(line, "installdir") {
                    let candidate = install_dir.join(name);
                    if valid_installation(&candidate) { found.insert(candidate); }
                }
            }
        }
    }
    found.into_iter().collect()
}

pub fn valid_installation(folder: &Path) -> bool {
    let data = if folder.join("data").is_dir() { folder.join("data") } else { folder.to_path_buf() };
    data.join("core/corehap.dat").is_file() || data.join("data009.cpk").is_file()
        || data.join("data000.cpk").is_file()
}

fn vdf_path(line: &str) -> Option<String> {
    vdf_value(line, "path").or_else(|| {
        let words = quoted(line);
        if words.len()==2 && words[0].parse::<usize>().is_ok() { Some(words[1].clone()) }
        else { None }
    })
}

fn vdf_value(line: &str, key: &str) -> Option<String> {
    let words = quoted(line);
    if words.len()==2 && words[0].eq_ignore_ascii_case(key) { Some(words[1].clone()) }
    else { None }
}

fn quoted(line: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' { continue; }
        let mut word = String::new();
        while let Some(ch) = chars.next() {
            if ch == '"' { break; }
            if ch == '\\' && chars.peek()==Some(&'\\') { chars.next(); word.push('\\'); }
            else { word.push(ch); }
        }
        result.push(word);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn vdf() {
        assert_eq!(vdf_value(r#""installdir" "NieRAutomata""#, "installdir"),Some("NieRAutomata".into()));
        assert_eq!(vdf_path(r#""path" "D:\\Games\\SteamLibrary""#),Some(r"D:\Games\SteamLibrary".into()));
    }
}
