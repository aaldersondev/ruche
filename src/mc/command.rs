//! Construction de la ligne de commande Java, classpath et natives compris.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::version::{Features, Version, lib_rules, os_name, rules_allow};
use crate::config::{Account, Settings};
use crate::i18n::{Strings, fill};

/// Fichier attendu par le jeu mais absent du disque.
#[derive(Clone, Debug)]
pub struct Missing {
    pub path: PathBuf,
    pub url: Option<String>,
}

/// Tout ce qui varie d'une instance a l'autre.
pub struct LaunchOptions {
    /// Textes dans la langue choisie, pour les rares erreurs remontees d'ici.
    pub s: &'static Strings,
    pub game_dir: PathBuf,
    pub natives_dir: PathBuf,
    pub java: PathBuf,
    pub xmx_mb: u64,
    pub xms_mb: u64,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub server: Option<(String, u16)>,
    pub extra_jvm: Vec<String>,
}

impl LaunchOptions {
    /// Options d'une instance a partir des reglages generaux.
    pub fn from_settings(settings: &Settings, account: &Account, version: &Version) -> Self {
        Self {
            s: settings.s(),
            game_dir: account.game_dir(settings),
            natives_dir: version.natives_dir(&settings.mc_dir),
            java: super::java::find_java(version, false),
            xmx_mb: account.xmx_mb.unwrap_or(settings.xmx_mb),
            xms_mb: settings
                .xms_mb
                .min(account.xmx_mb.unwrap_or(settings.xmx_mb)),
            width: settings.width,
            height: settings.height,
            fullscreen: settings.fullscreen,
            server: settings.server_host_port(),
            extra_jvm: settings.extra_jvm_args(),
        }
    }

    fn features(&self) -> Features {
        Features {
            demo: false,
            custom_resolution: self.width > 0 && self.height > 0,
            quick_play_multiplayer: self.server.is_some(),
        }
    }
}

/// Convertit `group:artifact:version[:classifier]` en chemin de depot maven.
pub fn maven_path(name: &str) -> PathBuf {
    let parts: Vec<&str> = name.split(':').collect();
    if parts.len() < 3 {
        return PathBuf::from(name);
    }
    let (group, artifact) = (parts[0], parts[1]);
    let mut version = parts[2].to_string();
    let mut classifier = parts.get(3).map(|s| s.to_string());
    let mut ext = "jar".to_string();
    if let Some((base, e)) = version.clone().split_once('@') {
        version = base.to_string();
        ext = e.to_string();
    }
    if let Some(c) = classifier.clone()
        && let Some((base, e)) = c.split_once('@')
    {
        classifier = Some(base.to_string());
        ext = e.to_string();
    }
    let file = match &classifier {
        Some(c) => format!("{artifact}-{version}-{c}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };
    let mut path = PathBuf::new();
    for segment in group.split('.') {
        path.push(segment);
    }
    path.push(artifact);
    path.push(version);
    path.push(file);
    path
}

/// Classifier natives de cette plateforme, pour les versions <= 1.18.
fn natives_classifier(lib: &Value) -> Option<String> {
    let key = lib.get("natives")?.get(os_name())?.as_str()?;
    Some(key.replace("${arch}", "64"))
}

fn artifact_of(mc_dir: &Path, lib: &Value) -> (PathBuf, Option<String>) {
    if let Some(artifact) = lib.get("downloads").and_then(|d| d.get("artifact"))
        && let Some(rel) = artifact.get("path").and_then(Value::as_str)
    {
        let url = artifact
            .get("url")
            .and_then(Value::as_str)
            .map(str::to_string);
        return (mc_dir.join("libraries").join(rel), url);
    }
    let name = lib.get("name").and_then(Value::as_str).unwrap_or_default();
    let rel = maven_path(name);
    let url = lib.get("url").and_then(Value::as_str).map(|base| {
        format!(
            "{}/{}",
            base.trim_end_matches('/'),
            rel.to_string_lossy().replace('\\', "/")
        )
    });
    (mc_dir.join("libraries").join(rel), url)
}

/// Jars du classpath, et ce qui manque a l'appel.
pub fn classpath(
    mc_dir: &Path,
    version: &Version,
    features: &Features,
) -> (Vec<PathBuf>, Vec<Missing>) {
    let mut seen: HashSet<String> = HashSet::new();
    let mut jars = Vec::new();
    let mut missing = Vec::new();

    for lib in version.libraries() {
        if !rules_allow(lib_rules(lib), features) {
            continue;
        }
        let name = lib.get("name").and_then(Value::as_str).unwrap_or_default();
        // group:artifact[:classifier] : la premiere vue gagne, donc celle de
        // l'enfant (Forge/Fabric) plutot que celle du parent.
        let parts: Vec<&str> = name.split(':').collect();
        let key = match parts.len() {
            0 | 1 => name.to_string(),
            2 | 3 => parts[..2].join(":"),
            _ => format!("{}:{}:{}", parts[0], parts[1], parts[3]),
        };
        if !seen.insert(key) {
            continue;
        }
        if natives_classifier(lib).is_some() {
            continue; // extrait, pas mis au classpath
        }
        let (path, url) = artifact_of(mc_dir, lib);
        if path.is_file() {
            jars.push(path);
        } else {
            missing.push(Missing { path, url });
        }
    }

    match &version.jar {
        Some(jar) => jars.push(jar.clone()),
        None => missing.push(Missing {
            path: mc_dir
                .join("versions")
                .join(&version.root)
                .join(format!("{}.jar", version.root)),
            url: None,
        }),
    }
    (jars, missing)
}

/// Extrait les natives des versions <= 1.18, une seule fois par version.
pub fn extract_natives(
    mc_dir: &Path,
    version: &Version,
    features: &Features,
    dest: &Path,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    let stamp = dest.join(".extracted");
    if stamp.is_file() {
        return Ok(());
    }
    for lib in version.libraries() {
        let Some(classifier) = natives_classifier(lib) else {
            continue;
        };
        if !rules_allow(lib_rules(lib), features) {
            continue;
        }
        let name = lib.get("name").and_then(Value::as_str).unwrap_or_default();
        let jar = lib
            .get("downloads")
            .and_then(|d| d.get("classifiers"))
            .and_then(|c| c.get(&classifier))
            .and_then(|c| c.get("path"))
            .and_then(Value::as_str)
            .map(|rel| mc_dir.join("libraries").join(rel))
            .unwrap_or_else(|| {
                mc_dir
                    .join("libraries")
                    .join(maven_path(&format!("{name}:{classifier}")))
            });
        if !jar.is_file() {
            continue;
        }
        let excludes: Vec<String> = lib
            .get("extract")
            .and_then(|e| e.get("exclude"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let file = std::fs::File::open(&jar)?;
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(_) => continue,
        };
        for i in 0..archive.len() {
            let mut entry = match archive.by_index(i) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let entry_name = entry.name().to_string();
            if entry.is_dir()
                || entry_name.starts_with("META-INF")
                || excludes.iter().any(|e| entry_name.starts_with(e))
            {
                continue;
            }
            let Some(file_name) = Path::new(&entry_name).file_name() else {
                continue;
            };
            let target = dest.join(file_name);
            if target.is_file() {
                continue;
            }
            if let Ok(mut out) = std::fs::File::create(&target) {
                let _ = std::io::copy(&mut entry, &mut out);
            }
        }
    }
    std::fs::File::create(stamp)?;
    Ok(())
}

/// Recupere les libraries absentes quand le json donne une URL.
pub fn download_missing(
    missing: &[Missing],
    s: &'static Strings,
    mut progress: impl FnMut(String),
) -> Vec<PathBuf> {
    let mut failed = Vec::new();
    for item in missing {
        let Some(url) = &item.url else {
            failed.push(item.path.clone());
            continue;
        };
        let name = item
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        progress(fill(s.log_downloading, &[&name]));
        if let Err(err) = fetch(url, &item.path) {
            progress(format!("{name} : {err}"));
            failed.push(item.path.clone());
        }
    }
    failed
}

fn fetch(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut response = ureq::get(url).call().map_err(|e| e.to_string())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(512 * 1024 * 1024)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    std::fs::write(dest, bytes).map_err(|e| e.to_string())
}

/// Construit la commande complete : java, options JVM, classe principale, arguments du jeu.
pub fn build(
    mc_dir: &Path,
    version: &Version,
    account: &Account,
    opts: &LaunchOptions,
) -> Result<(Vec<String>, Vec<Missing>), String> {
    let features = opts.features();
    let (jars, missing) = classpath(mc_dir, version, &features);
    extract_natives(mc_dir, version, &features, &opts.natives_dir)
        .map_err(|e| format!("natives : {e}"))?;
    std::fs::create_dir_all(&opts.game_dir)
        .map_err(|e| fill(opts.s.log_instance_dir, &[&e.to_string()]))?;

    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath_str = jars
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(separator);

    let assets_dir = if matches!(version.assets.as_str(), "pre-1.6" | "legacy") {
        mc_dir.join("assets/virtual/legacy")
    } else {
        mc_dir.join("assets")
    };
    let _ = std::fs::create_dir_all(&assets_dir);

    let token = if account.access_token.is_empty() {
        "0".to_string()
    } else {
        account.access_token.clone()
    };
    let uuid_plain = account.uuid.replace('-', "");
    let subst: Vec<(&str, String)> = vec![
        ("auth_player_name", account.name.clone()),
        ("auth_uuid", account.uuid.clone()),
        ("auth_access_token", token.clone()),
        ("auth_session", format!("token:{token}:{uuid_plain}")),
        ("auth_xuid", account.xuid.clone()),
        ("clientid", account.client_id.clone()),
        ("user_type", account.user_type().to_string()),
        ("user_properties", "{}".to_string()),
        ("version_name", version.id.clone()),
        ("version_type", version.version_type.clone()),
        ("game_directory", path_str(&opts.game_dir)),
        ("assets_root", path_str(&assets_dir)),
        ("game_assets", path_str(&assets_dir)),
        ("assets_index_name", version.asset_index.clone()),
        ("natives_directory", path_str(&opts.natives_dir)),
        ("launcher_name", "ruche".to_string()),
        ("launcher_version", env!("CARGO_PKG_VERSION").to_string()),
        ("classpath", classpath_str.clone()),
        ("classpath_separator", separator.to_string()),
        ("library_directory", path_str(&mc_dir.join("libraries"))),
        ("resolution_width", opts.width.to_string()),
        ("resolution_height", opts.height.to_string()),
        ("quickPlayPath", String::new()),
        (
            "quickPlayMultiplayer",
            opts.server
                .as_ref()
                .map(|(h, p)| format!("{h}:{p}"))
                .unwrap_or_default(),
        ),
    ];
    let expand = |text: &str| -> String {
        let mut out = text.to_string();
        for (key, value) in &subst {
            if out.contains('$') {
                out = out.replace(&format!("${{{key}}}"), value);
            }
        }
        out
    };

    let mut cmd = vec![path_str(&opts.java)];
    cmd.push(format!("-Xmx{}M", opts.xmx_mb));
    cmd.push(format!("-Xms{}M", opts.xms_mb.max(256)));
    // Un tas qui ne se preallouhe pas et des pauses courtes : plusieurs clients
    // doivent tenir sans envoyer la machine dans le fichier d'echange.
    cmd.push("-XX:+UseG1GC".into());
    cmd.push("-XX:MaxGCPauseMillis=50".into());
    cmd.push("-XX:-AlwaysPreTouch".into());
    cmd.push(format!(
        "-XX:MaxMetaspaceSize={}M",
        if version.java_major <= 8 { 256 } else { 512 }
    ));
    if version.java_major >= 9 {
        cmd.push("-XX:+UnlockExperimentalVMOptions".into());
        cmd.push("-XX:G1NewSizePercent=20".into());
        cmd.push("-XX:G1ReservePercent=20".into());
        cmd.push("-XX:G1HeapRegionSize=16M".into());
        cmd.push("-XX:+UseStringDeduplication".into());
    }
    let tmp_dir = opts.game_dir.join(".tmp");
    let _ = std::fs::create_dir_all(&tmp_dir);
    cmd.push(format!("-Djna.tmpdir={}", path_str(&tmp_dir)));
    cmd.push(format!(
        "-Dorg.lwjgl.system.SharedLibraryExtractPath={}",
        path_str(&opts.natives_dir)
    ));
    cmd.push(format!(
        "-Dio.netty.native.workdir={}",
        path_str(&opts.natives_dir)
    ));
    cmd.push("-Dfml.ignoreInvalidMinecraftCertificates=true".into());
    cmd.push("-Dfml.ignorePatchDiscrepancies=true".into());
    cmd.extend(opts.extra_jvm.iter().cloned());

    match version.arguments("jvm") {
        Some(items) => cmd.extend(collect(items, &features, &expand)),
        None => {
            cmd.push(format!(
                "-Djava.library.path={}",
                path_str(&opts.natives_dir)
            ));
            cmd.push("-cp".into());
            cmd.push(classpath_str);
        }
    }

    cmd.push(version.main_class.clone());

    let mut game: Vec<String> = match version.arguments("game") {
        Some(items) => collect(items, &features, &expand),
        None => version
            .legacy_arguments()
            .unwrap_or_default()
            .split_whitespace()
            .map(&expand)
            .collect(),
    };
    if opts.width > 0 && opts.height > 0 && !game.iter().any(|a| a == "--width") {
        game.push("--width".into());
        game.push(opts.width.to_string());
        game.push("--height".into());
        game.push(opts.height.to_string());
    }
    if opts.fullscreen && !game.iter().any(|a| a == "--fullscreen") {
        game.push("--fullscreen".into());
    }
    if let Some((host, port)) = &opts.server {
        if version.supports_quick_play() {
            if !game.iter().any(|a| a == "--quickPlayMultiplayer") {
                game.push("--quickPlayMultiplayer".into());
                game.push(format!("{host}:{port}"));
            }
        } else if !game.iter().any(|a| a == "--server") {
            game.push("--server".into());
            game.push(host.clone());
            game.push("--port".into());
            game.push(port.to_string());
        }
    }
    cmd.extend(game);
    Ok((cmd, missing))
}

/// Applatit une liste d'arguments json (chaines et blocs conditionnels).
fn collect(items: &[Value], features: &Features, expand: &impl Fn(&str) -> String) -> Vec<String> {
    let mut out = Vec::new();
    for item in items {
        match item {
            Value::String(text) => out.push(expand(text)),
            Value::Object(map) => {
                let rules = map.get("rules").or_else(|| map.get("compatibilityRules"));
                if !rules_allow(rules, features) {
                    continue;
                }
                match map.get("value").or_else(|| map.get("values")) {
                    Some(Value::String(text)) => out.push(expand(text)),
                    Some(Value::Array(values)) => {
                        out.extend(values.iter().filter_map(Value::as_str).map(&expand));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

fn path_str(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// Reglages graphiques legers pour une instance qui vient d'etre creee.
pub const LOW_OPTIONS: [(&str, &str); 26] = [
    ("renderDistance", "3"),
    ("simulationDistance", "5"),
    ("maxFps", "60"),
    ("graphicsMode", "0"),
    ("fancyGraphics", "false"),
    ("ao", "false"),
    ("particles", "2"),
    ("entityShadows", "false"),
    ("enableVsync", "false"),
    ("bobView", "false"),
    ("guiScale", "2"),
    ("fullscreen", "false"),
    ("pauseOnLostFocus", "false"),
    ("soundCategory_master", "0.0"),
    ("soundCategory_music", "0.0"),
    ("renderClouds", "false"),
    ("useVbo", "true"),
    ("mipmapLevels", "0"),
    ("biomeBlendRadius", "0"),
    ("screenEffectScale", "0.0"),
    ("glintSpeed", "0.0"),
    ("narrator", "0"),
    ("showSubtitles", "false"),
    ("darkMojangStudiosBackground", "true"),
    ("skipMultiplayerWarning", "true"),
    ("joinedFirstServer", "true"),
];

/// Ecrit `options.txt` si l'instance n'en a pas encore.
pub fn seed_options(game_dir: &Path, low: bool) -> std::io::Result<bool> {
    let path = game_dir.join("options.txt");
    if path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(game_dir)?;
    let mut file = std::fs::File::create(path)?;
    if low {
        for (key, value) in LOW_OPTIONS {
            writeln!(file, "{key}:{value}")?;
        }
    } else {
        writeln!(file, "pauseOnLostFocus:false")?;
        writeln!(file, "skipMultiplayerWarning:true")?;
    }
    Ok(true)
}

/// Ajoute le serveur a la liste multijoueur (NBT non compresse).
pub fn write_servers_dat(
    game_dir: &Path,
    host: &str,
    port: u16,
    name: &str,
) -> std::io::Result<bool> {
    let path = game_dir.join("servers.dat");
    if path.exists() {
        return Ok(false);
    }
    let ip = if port == 25565 {
        host.to_string()
    } else {
        format!("{host}:{port}")
    };

    fn tag_string(text: &str, out: &mut Vec<u8>) {
        let bytes = text.as_bytes();
        out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        out.extend_from_slice(bytes);
    }

    let mut entry = Vec::new();
    entry.push(0x08); // TAG_String
    tag_string("ip", &mut entry);
    tag_string(&ip, &mut entry);
    entry.push(0x08);
    tag_string("name", &mut entry);
    tag_string(name, &mut entry);
    entry.push(0x00); // fin du compound

    let mut out = Vec::new();
    out.push(0x0a); // compound racine
    tag_string("", &mut out);
    out.push(0x09); // TAG_List
    tag_string("servers", &mut out);
    out.push(0x0a); // ... de compounds
    out.extend_from_slice(&1i32.to_be_bytes());
    out.extend_from_slice(&entry);
    out.push(0x00); // fin du compound racine

    std::fs::create_dir_all(game_dir)?;
    std::fs::write(path, out)?;
    Ok(true)
}

/// Jonction NTFS (ou lien symbolique) pour partager mods et resourcepacks.
pub fn link_dir(src: &Path, dst: &Path) -> bool {
    if dst.exists() {
        return true;
    }
    if !src.is_dir() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let status = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(dst)
            .arg(src)
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .output();
        status.is_ok() && dst.exists()
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(src, dst).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_names_become_paths() {
        assert_eq!(
            maven_path("com.mojang:netty:1.8.8"),
            PathBuf::from("com/mojang/netty/1.8.8/netty-1.8.8.jar")
        );
        assert_eq!(
            maven_path("org.lwjgl:lwjgl:3.3.3:natives-windows"),
            PathBuf::from("org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-windows.jar")
        );
        assert_eq!(
            maven_path("net.minecraftforge:forge:1.20.1@zip"),
            PathBuf::from("net/minecraftforge/forge/1.20.1/forge-1.20.1.zip")
        );
    }

    #[test]
    fn servers_dat_is_valid_nbt() {
        let dir = std::env::temp_dir().join("ruche-test-servers");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(write_servers_dat(&dir, "51.38.34.244", 25565, "Kronia").unwrap());
        let raw = std::fs::read(dir.join("servers.dat")).unwrap();
        // compound racine sans nom, contenant une liste "servers" d'un element
        assert_eq!(raw[0], 0x0a);
        assert_eq!(&raw[1..3], &[0, 0]);
        assert_eq!(raw[3], 0x09);
        assert_eq!(&raw[4..6], &(7u16).to_be_bytes());
        assert_eq!(&raw[6..13], b"servers");
        assert_eq!(raw[13], 0x0a);
        assert_eq!(&raw[14..18], &1i32.to_be_bytes());
        assert_eq!(*raw.last().unwrap(), 0x00);
        assert!(String::from_utf8_lossy(&raw).contains("51.38.34.244"));
        // deuxieme appel : on ne touche pas a un fichier existant
        assert!(!write_servers_dat(&dir, "autre", 25565, "Autre").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn options_are_only_seeded_once() {
        let dir = std::env::temp_dir().join("ruche-test-options");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(seed_options(&dir, true).unwrap());
        let text = std::fs::read_to_string(dir.join("options.txt")).unwrap();
        assert!(text.contains("renderDistance:3"));
        assert!(text.contains("pauseOnLostFocus:false"));
        assert!(!seed_options(&dir, true).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
