//! Choix du JRE : les runtimes du launcher officiel d'abord, les JDK du
//! systeme ensuite, le `java` du PATH en dernier recours.

use std::path::{Path, PathBuf};

use super::version::Version;

/// Executable a lancer : `javaw.exe` masque la console noire sous Windows.
fn exe_name(console: bool) -> &'static str {
    if cfg!(windows) {
        if console { "java.exe" } else { "javaw.exe" }
    } else {
        "java"
    }
}

fn runtime_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from(r"C:\Program Files (x86)\Minecraft Launcher\runtime"),
        PathBuf::from(r"C:\Program Files\Minecraft Launcher\runtime"),
    ];
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        roots.push(
            PathBuf::from(local)
                .join("Packages")
                .join("Microsoft.4297127D64EC6_8wekyb3d8bbwe")
                .join("LocalCache/Local/runtime"),
        );
    }
    roots
}

const COMPONENTS: [(&str, u32); 6] = [
    ("jre-legacy", 8),
    ("java-runtime-alpha", 16),
    ("java-runtime-beta", 17),
    ("java-runtime-gamma", 17),
    ("java-runtime-delta", 21),
    ("java-runtime-epsilon", 25),
];

/// Trouve un JRE compatible avec la version demandee.
pub fn find_java(version: &Version, console: bool) -> PathBuf {
    let exe = exe_name(console);
    let major = version.java_major;

    // 1. le composant exact reclame par le json, puis ceux du meme major
    let mut wanted: Vec<String> = Vec::new();
    if let Some(component) = &version.java_component {
        wanted.push(component.clone());
    }
    wanted.extend(
        COMPONENTS
            .iter()
            .filter(|(name, m)| *m == major && Some(*name) != version.java_component.as_deref())
            .map(|(name, _)| name.to_string()),
    );

    for root in runtime_roots() {
        for component in &wanted {
            for arch in ["windows-x64", "windows-x86", "windows-arm64"] {
                let candidate = root
                    .join(component)
                    .join(arch)
                    .join(component)
                    .join("bin")
                    .join(exe);
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
    }

    // 2. les JDK installes sur la machine
    for base in [
        r"C:\Program Files\Java",
        r"C:\Program Files\Eclipse Adoptium",
        r"C:\Program Files\Microsoft",
        r"C:\Program Files\Zulu",
        "/usr/lib/jvm",
    ] {
        if let Some(found) = scan_jdk_dir(Path::new(base), major, exe) {
            return found;
        }
    }

    // 3. le java du PATH
    which(exe).unwrap_or_else(|| PathBuf::from(exe))
}

/// Cherche un dossier de JDK dont le nom contient le numero de version voulu.
fn scan_jdk_dir(base: &Path, major: u32, exe: &str) -> Option<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    names.reverse(); // a defaut, la revision la plus recente
    for name in names {
        if !mentions_major(&name, major) {
            continue;
        }
        let candidate = base.join(&name).join("bin").join(exe);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `jdk-21`, `jdk-21.0.4`, `jre1.8.0_471`... sans confondre 21 et 121.
fn mentions_major(dir_name: &str, major: u32) -> bool {
    if major == 8 && (dir_name.contains("1.8") || dir_name.contains("-8")) {
        return true;
    }
    let target = major.to_string();
    let bytes: Vec<char> = dir_name.chars().collect();
    let needle: Vec<char> = target.chars().collect();
    for start in 0..bytes.len() {
        if bytes[start..].starts_with(&needle[..]) {
            let before_ok = start == 0 || !bytes[start - 1].is_ascii_digit();
            let after = start + needle.len();
            let after_ok = after >= bytes.len() || !bytes[after].is_ascii_digit();
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

fn which(exe: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(exe))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn major_numbers_are_not_confused() {
        assert!(mentions_major("jdk-21", 21));
        assert!(mentions_major("jdk-21.0.4+7", 21));
        assert!(!mentions_major("jdk-121", 21));
        assert!(mentions_major("jre1.8.0_471", 8));
        assert!(mentions_major("jdk-17.0.17.10-hotspot", 17));
        assert!(!mentions_major("jdk-17", 7));
    }
}
