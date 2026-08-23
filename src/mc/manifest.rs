//! Catalogue des versions : celles installées, et toutes celles que Mojang
//! propose.
//!
//! Le manifeste est mis en cache sur le disque : sans réseau, le launcher
//! garde la liste complète et se contente de dire ce qu'il ne peut pas
//! télécharger.

use std::path::{Path, PathBuf};

use serde_json::Value;

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Release,
    Snapshot,
    Old,
    /// Profil local sans équivalent chez Mojang : Forge, Fabric, OptiFine…
    Modded,
}

impl Kind {
    fn from_str(raw: &str) -> Self {
        match raw {
            "release" => Kind::Release,
            "snapshot" => Kind::Snapshot,
            _ => Kind::Old,
        }
    }
}

/// Une entrée du sélecteur de version.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    /// Adresse du json de version chez Mojang ; absente pour un profil local.
    pub url: Option<String>,
    /// Le json est présent dans `.minecraft/versions`.
    pub installed: bool,
    /// Le jar client est là : la version est jouable hors ligne.
    pub ready: bool,
}

pub struct Catalog {
    pub entries: Vec<Entry>,
    pub latest_release: String,
    pub latest_snapshot: String,
    /// Vrai si la liste distante vient du réseau ou d'un cache, faux si on
    /// n'a que le contenu de `.minecraft`.
    pub remote_known: bool,
}

impl Catalog {
    /// Catalogue sans rien : ce que voit la file avant que l'interface lui
    /// passe le vrai.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            latest_release: String::new(),
            latest_snapshot: String::new(),
            remote_known: false,
        }
    }

    /// Construit le catalogue à partir du disque : versions installées et
    /// manifeste en cache.
    pub fn load(mc_dir: &Path, cache_dir: &Path) -> Self {
        let remote = read_cached(cache_dir, mc_dir);
        Self::build(mc_dir, remote)
    }

    /// Va chercher le manifeste à jour, puis reconstruit le catalogue.
    pub fn refresh(mc_dir: &Path, cache_dir: &Path) -> Result<Self, String> {
        let text = fetch_manifest()?;
        let value: Value =
            serde_json::from_str(&text).map_err(|e| format!("manifeste illisible : {e}"))?;
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(cache_path(cache_dir), &text);
        Ok(Self::build(mc_dir, Some(value)))
    }

    fn build(mc_dir: &Path, remote: Option<Value>) -> Self {
        let installed = super::version::list_versions(mc_dir);
        let mut entries: Vec<Entry> = Vec::new();
        let mut latest_release = String::new();
        let mut latest_snapshot = String::new();
        let remote_known = remote.is_some();

        if let Some(manifest) = &remote {
            latest_release = manifest
                .get("latest")
                .and_then(|l| l.get("release"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            latest_snapshot = manifest
                .get("latest")
                .and_then(|l| l.get("snapshot"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if let Some(list) = manifest.get("versions").and_then(Value::as_array) {
                for item in list {
                    let (Some(id), Some(url)) = (
                        item.get("id").and_then(Value::as_str),
                        item.get("url").and_then(Value::as_str),
                    ) else {
                        continue;
                    };
                    let kind = Kind::from_str(
                        item.get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("release"),
                    );
                    entries.push(Entry {
                        id: id.to_string(),
                        kind,
                        url: Some(url.to_string()),
                        installed: false,
                        ready: false,
                    });
                }
            }
        }

        // Les profils locaux qui ne figurent pas au manifeste (Forge, Fabric…)
        // rejoignent la liste ; les autres sont simplement marqués installés.
        for id in installed {
            let ready = jar_present(mc_dir, &id);
            match entries.iter_mut().find(|e| e.id == id) {
                Some(entry) => {
                    entry.installed = true;
                    entry.ready = ready;
                }
                None => entries.push(Entry {
                    id,
                    kind: Kind::Modded,
                    url: None,
                    installed: true,
                    ready,
                }),
            }
        }

        // Les versions installées d'abord, puis le reste par ordre décroissant.
        entries.sort_by(|a, b| {
            b.installed
                .cmp(&a.installed)
                .then_with(|| super::version::sort_key(&a.id).cmp(&super::version::sort_key(&b.id)))
        });

        Self {
            entries,
            latest_release,
            latest_snapshot,
            remote_known,
        }
    }

    pub fn find(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Adresse du json de version chez Mojang, si on la connaît.
    pub fn url_of(&self, id: &str) -> Option<String> {
        self.find(id).and_then(|e| e.url.clone())
    }

    pub fn installed_count(&self) -> usize {
        self.entries.iter().filter(|e| e.installed).count()
    }
}

/// Le jar client de cette version (ou d'un de ses parents) est sur le disque.
fn jar_present(mc_dir: &Path, id: &str) -> bool {
    super::version::resolve(mc_dir, id)
        .map(|v| v.jar.is_some())
        .unwrap_or(false)
}

fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("version_manifest_v2.json")
}

/// Notre cache d'abord, celui du launcher officiel ensuite.
fn read_cached(cache_dir: &Path, mc_dir: &Path) -> Option<Value> {
    for path in [
        cache_path(cache_dir),
        mc_dir.join("versions").join("version_manifest_v2.json"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(value) = serde_json::from_str::<Value>(&text)
            && value.get("versions").is_some()
        {
            return Some(value);
        }
    }
    None
}

fn fetch_manifest() -> Result<String, String> {
    let mut response = ureq::get(MANIFEST_URL)
        .call()
        .map_err(|e| format!("{MANIFEST_URL} : {e}"))?;
    response
        .body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Value {
        json!({
            "latest": {"release": "1.20.1", "snapshot": "24w01a"},
            "versions": [
                {"id": "24w01a", "type": "snapshot", "url": "https://exemple/24w01a.json"},
                {"id": "1.20.1", "type": "release", "url": "https://exemple/1.20.1.json"},
                {"id": "1.8.9", "type": "release", "url": "https://exemple/1.8.9.json"},
                {"id": "b1.7.3", "type": "old_beta", "url": "https://exemple/b1.7.3.json"}
            ]
        })
    }

    #[test]
    fn remote_and_local_are_merged() {
        let dir = std::env::temp_dir().join("ruche-test-catalogue");
        let _ = std::fs::remove_dir_all(&dir);
        // une version vanilla installee et un profil local absent du manifeste
        for (id, parent) in [("1.8.9", None), ("fabric-1.20.1", Some("1.20.1"))] {
            let vdir = dir.join("versions").join(id);
            std::fs::create_dir_all(&vdir).unwrap();
            let body = match parent {
                Some(p) => json!({"mainClass": "X", "inheritsFrom": p}),
                None => json!({"mainClass": "X"}),
            };
            std::fs::write(vdir.join(format!("{id}.json")), body.to_string()).unwrap();
        }

        let catalog = Catalog::build(&dir, Some(manifest()));
        assert!(catalog.remote_known);
        assert_eq!(catalog.latest_release, "1.20.1");
        assert_eq!(catalog.entries.len(), 5, "4 distantes + 1 profil local");

        let local = catalog.find("1.8.9").unwrap();
        assert!(local.installed, "la version locale doit etre marquee");
        assert!(local.url.is_some(), "elle reste telechargeable");

        let modded = catalog.find("fabric-1.20.1").unwrap();
        assert_eq!(modded.kind, Kind::Modded);
        assert!(modded.installed && modded.url.is_none());

        let remote_only = catalog.find("24w01a").unwrap();
        assert!(!remote_only.installed);
        assert_eq!(remote_only.kind, Kind::Snapshot);

        // les installees passent devant
        let first_two: Vec<&str> = catalog.entries[..2].iter().map(|e| e.id.as_str()).collect();
        assert!(first_two.contains(&"1.8.9") && first_two.contains(&"fabric-1.20.1"));
        assert_eq!(catalog.installed_count(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn without_a_manifest_only_local_versions_show_up() {
        let dir = std::env::temp_dir().join("ruche-test-catalogue-hors-ligne");
        let _ = std::fs::remove_dir_all(&dir);
        let vdir = dir.join("versions").join("1.12.2");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(
            vdir.join("1.12.2.json"),
            json!({"mainClass": "X"}).to_string(),
        )
        .unwrap();

        let catalog = Catalog::build(&dir, None);
        assert!(!catalog.remote_known);
        assert_eq!(catalog.entries.len(), 1);
        assert!(catalog.entries[0].installed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
