//! Reglages et comptes, tels qu'ils sont ecrits sur le disque.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::i18n::{Lang, Strings};

/// Dossier de configuration : `%APPDATA%\Ruche` sur Windows.
pub fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("Ruche");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".config/ruche");
        }
    }
    PathBuf::from(".")
}

/// Emplacement par defaut de `.minecraft`.
pub fn default_mc_dir() -> PathBuf {
    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join(".minecraft");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".minecraft");
        }
    }
    PathBuf::from(".minecraft")
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Normal,
    #[default]
    BelowNormal,
    Idle,
}

impl Priority {
    pub const ALL: [Priority; 3] = [Priority::Normal, Priority::BelowNormal, Priority::Idle];

    pub fn label(self) -> &'static str {
        match self {
            Priority::Normal => "normale",
            Priority::BelowNormal => "basse",
            Priority::Idle => "inactive",
        }
    }

    /// Drapeau de creation de process Windows correspondant.
    pub fn creation_flag(self) -> u32 {
        match self {
            Priority::Normal => 0x0000_0020,      // NORMAL_PRIORITY_CLASS
            Priority::BelowNormal => 0x0000_4000, // BELOW_NORMAL_PRIORITY_CLASS
            Priority::Idle => 0x0000_0040,        // IDLE_PRIORITY_CLASS
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct Settings {
    pub mc_dir: PathBuf,
    pub instances_dir: PathBuf,
    pub version: String,
    /// Tas maximum de chaque client, en mebioctets.
    pub xmx_mb: u64,
    pub xms_mb: u64,
    pub max_instances: usize,
    /// RAM physique qu'on refuse d'entamer.
    pub reserve_mb: u64,
    /// Surcout estime hors tas : JVM, pilote graphique, natives.
    pub overhead_mb: u64,
    pub stagger_min_s: u64,
    pub stagger_max_s: u64,
    pub wait_timeout_s: u64,
    pub cores_per_instance: usize,
    pub priority: Priority,
    pub width: u32,
    pub height: u32,
    pub fullscreen: bool,
    pub low_settings: bool,
    pub share_mods: bool,
    pub add_server_entry: bool,
    pub ignore_ram_guard: bool,
    pub server: String,
    pub server_name: String,
    pub extra_jvm: String,
    pub azure_client_id: String,
    pub lang: Lang,
    pub discord_enabled: bool,
    pub discord_app_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            mc_dir: default_mc_dir(),
            instances_dir: config_dir().join("instances"),
            version: String::new(),
            xmx_mb: 2048,
            xms_mb: 512,
            max_instances: 4,
            reserve_mb: 3000,
            overhead_mb: 700,
            stagger_min_s: 8,
            stagger_max_s: 90,
            wait_timeout_s: 300,
            cores_per_instance: 4,
            priority: Priority::BelowNormal,
            width: 854,
            height: 480,
            fullscreen: false,
            low_settings: true,
            share_mods: true,
            add_server_entry: true,
            ignore_ram_guard: false,
            server: String::new(),
            server_name: "Serveur".into(),
            extra_jvm: String::new(),
            azure_client_id: String::new(),
            lang: Lang::default(),
            discord_enabled: false,
            discord_app_id: String::new(),
        }
    }
}

impl Settings {
    /// Textes dans la langue choisie.
    pub fn s(&self) -> &'static Strings {
        self.lang.strings()
    }

    pub fn path() -> PathBuf {
        config_dir().join("settings.json")
    }

    pub fn load() -> Self {
        read_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        write_json(&Self::path(), self)
    }

    /// Combien d'instances de plus tiennent en RAM tout de suite.
    pub fn room_for_more(&self) -> (usize, u64) {
        let (_total, avail) = crate::sys::memory_mb();
        let per = self.xmx_mb + self.overhead_mb;
        let free = avail.saturating_sub(self.reserve_mb);
        ((free / per.max(1)) as usize, avail)
    }

    pub fn extra_jvm_args(&self) -> Vec<String> {
        self.extra_jvm
            .split_whitespace()
            .map(str::to_string)
            .collect()
    }

    /// Serveur saisi sous la forme `hote:port`.
    pub fn server_host_port(&self) -> Option<(String, u16)> {
        let raw = self.server.trim();
        if raw.is_empty() {
            return None;
        }
        match raw.rsplit_once(':') {
            Some((host, port)) => Some((host.to_string(), port.parse().unwrap_or(25565))),
            None => Some((raw.to_string(), 25565)),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum AccountKind {
    #[default]
    Offline,
    Microsoft,
}

impl AccountKind {
    pub fn label(self) -> &'static str {
        match self {
            AccountKind::Offline => "hors-ligne",
            AccountKind::Microsoft => "premium",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Account {
    pub name: String,
    pub uuid: String,
    pub kind: AccountKind,
    /// Jeton Minecraft (vide ou "0" pour un compte hors-ligne).
    pub access_token: String,
    /// Jeton de rafraichissement Microsoft, protege par DPAPI.
    pub refresh_token: String,
    pub xuid: String,
    pub client_id: String,
    /// Fin de validite du jeton Minecraft, en secondes epoch.
    pub expires_at: u64,
    /// Nom du dossier d'instance.
    pub instance: String,
    /// Version propre a ce compte ; sinon celle choisie en haut.
    pub version: Option<String>,
    /// Tas propre a ce compte, en mebioctets.
    pub xmx_mb: Option<u64>,
    pub selected: bool,
}

impl Account {
    pub fn offline(name: &str) -> Self {
        Self {
            name: name.to_string(),
            uuid: crate::auth::offline_uuid(name),
            kind: AccountKind::Offline,
            access_token: "0".into(),
            instance: sanitize(name),
            selected: true,
            ..Default::default()
        }
    }

    pub fn is_premium(&self) -> bool {
        self.kind == AccountKind::Microsoft
    }

    pub fn user_type(&self) -> &'static str {
        if self.is_premium() { "msa" } else { "legacy" }
    }

    /// Secondes restantes avant expiration de la session Minecraft.
    pub fn session_left(&self) -> i64 {
        self.expires_at as i64 - now_secs() as i64
    }

    pub fn game_dir(&self, settings: &Settings) -> PathBuf {
        let dir = if self.instance.is_empty() {
            sanitize(&self.name)
        } else {
            self.instance.clone()
        };
        settings.instances_dir.join(dir)
    }
}

pub fn accounts_path() -> PathBuf {
    config_dir().join("accounts.json")
}

pub fn load_accounts() -> Vec<Account> {
    read_json(&accounts_path()).unwrap_or_default()
}

pub fn save_accounts(accounts: &[Account]) -> std::io::Result<()> {
    write_json(&accounts_path(), accounts)
}

/// Rend un nom de dossier utilisable a partir d'un pseudo.
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "compte".into()
    } else {
        cleaned
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    // Un fichier retouche au Bloc-notes ou par PowerShell commence par un BOM :
    // sans ca, la configuration serait silencieusement remise a zero.
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_readable_names() {
        assert_eq!(sanitize("Alt_1"), "Alt_1");
        assert_eq!(sanitize("Julien(main)"), "Julien_main_");
        assert_eq!(sanitize(""), "compte");
    }

    #[test]
    fn server_parsing() {
        let mut s = Settings::default();
        assert_eq!(s.server_host_port(), None);
        s.server = "kronia.fr".into();
        assert_eq!(s.server_host_port(), Some(("kronia.fr".into(), 25565)));
        s.server = "51.38.34.244:25570".into();
        assert_eq!(s.server_host_port(), Some(("51.38.34.244".into(), 25570)));
    }

    #[test]
    fn settings_survive_a_roundtrip() {
        let s = Settings {
            xmx_mb: 3072,
            priority: Priority::Idle,
            ..Default::default()
        };
        let text = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn a_byte_order_mark_does_not_reset_everything() {
        let dir = std::env::temp_dir().join("ruche-test-bom");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "\u{feff}{\"xmx_mb\": 4096}").unwrap();
        let settings: Settings = read_json(&path).expect("fichier avec BOM illisible");
        assert_eq!(settings.xmx_mb, 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_or_missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"xmx_mb": 1024}"#).unwrap();
        assert_eq!(s.xmx_mb, 1024);
        assert_eq!(s.max_instances, Settings::default().max_instances);
    }
}
