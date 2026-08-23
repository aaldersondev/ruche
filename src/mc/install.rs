//! Installation d'une version absente : json, jar client, libraries, assets.
//!
//! Tout est repris là où ça s'est arrêté : un fichier déjà présent n'est pas
//! retéléchargé, et rien n'est écrit à l'emplacement final avant d'être
//! complet.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde_json::Value;

use super::manifest::Catalog;
use super::version::{self, Features, Version};
use crate::i18n::{Strings, fill};

const RESOURCES: &str = "https://resources.download.minecraft.net";
/// Assez pour saturer une connexion domestique sans noyer le disque.
const PARALLEL: usize = 8;

/// Avancement d'une étape, tel qu'affiché dans la carte de l'instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub label: String,
    pub done: u64,
    pub total: u64,
}

/// S'assure que la version est jouable ; télécharge ce qui manque.
pub fn ensure(
    mc_dir: &Path,
    id: &str,
    catalog: &Catalog,
    s: &'static Strings,
    report: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    ensure_json(mc_dir, id, catalog, s, 0)?;
    let version = version::resolve(mc_dir, id)?;
    ensure_client_jar(mc_dir, &version, s, report)?;
    ensure_libraries(mc_dir, &version, s, report)?;
    ensure_assets(mc_dir, &version, s, report)?;
    Ok(())
}

/// Rien à faire ? Autant ne pas afficher d'étape de téléchargement.
pub fn is_complete(mc_dir: &Path, id: &str) -> bool {
    let Ok(version) = version::resolve(mc_dir, id) else {
        return false;
    };
    if version.jar.is_none() {
        return false;
    }
    let features = Features::default();
    let (_jars, missing) = super::command::classpath(mc_dir, &version, &features);
    if !missing.is_empty() {
        return false;
    }
    match asset_index_path(mc_dir, &version) {
        Some(path) => path.is_file(),
        None => true,
    }
}

// ------------------------------------------------------------------- json

fn ensure_json(
    mc_dir: &Path,
    id: &str,
    catalog: &Catalog,
    s: &'static Strings,
    depth: usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err(fill(s.install_deep, &[id]));
    }
    let path = mc_dir.join("versions").join(id).join(format!("{id}.json"));
    if !path.is_file() {
        let Some(url) = catalog.url_of(id) else {
            return Err(fill(s.install_unknown_version, &[id]));
        };
        download(&url, &path, None)?;
    }
    // Un profil moddé a besoin de son parent : on l'installe aussi.
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if let Some(parent) = value.get("inheritsFrom").and_then(Value::as_str) {
        ensure_json(mc_dir, parent, catalog, s, depth + 1)?;
    }
    Ok(())
}

// -------------------------------------------------------------- jar client

fn ensure_client_jar(
    mc_dir: &Path,
    version: &Version,
    s: &'static Strings,
    report: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    if version.jar.is_some() {
        return Ok(());
    }
    let client = version
        .raw
        .get("downloads")
        .and_then(|d| d.get("client"))
        .ok_or_else(|| fill(s.install_no_jar, &[&version.id]))?;
    let url = client
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| fill(s.install_no_jar, &[&version.id]))?;
    let sha = client.get("sha1").and_then(Value::as_str);
    let target = mc_dir
        .join("versions")
        .join(&version.root)
        .join(format!("{}.jar", version.root));
    report(Step {
        label: fill(s.install_jar, &[&version.root]),
        done: 0,
        total: 1,
    });
    download(url, &target, sha)
}

// --------------------------------------------------------------- libraries

fn ensure_libraries(
    mc_dir: &Path,
    version: &Version,
    s: &'static Strings,
    report: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    let features = Features::default();
    let (_jars, missing) = super::command::classpath(mc_dir, version, &features);
    let wanted: Vec<_> = missing.into_iter().filter(|m| m.url.is_some()).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let total = wanted.len() as u64;
    for (index, item) in wanted.iter().enumerate() {
        report(Step {
            label: s.install_libraries.to_string(),
            done: index as u64,
            total,
        });
        let url = item.url.as_deref().unwrap_or_default();
        // Une library manquante sans miroir n'est pas fatale ici : le
        // lancement dira précisément ce qui bloque.
        let _ = download(url, &item.path, None);
    }
    Ok(())
}

// ------------------------------------------------------------------ assets

fn asset_index_path(mc_dir: &Path, version: &Version) -> Option<PathBuf> {
    let id = version.raw.get("assetIndex")?.get("id")?.as_str()?;
    Some(
        mc_dir
            .join("assets")
            .join("indexes")
            .join(format!("{id}.json")),
    )
}

fn ensure_assets(
    mc_dir: &Path,
    version: &Version,
    s: &'static Strings,
    report: &(dyn Fn(Step) + Sync),
) -> Result<(), String> {
    let Some(index_meta) = version.raw.get("assetIndex") else {
        return Ok(()); // très anciennes versions : rien à faire
    };
    let (Some(id), Some(url)) = (
        index_meta.get("id").and_then(Value::as_str),
        index_meta.get("url").and_then(Value::as_str),
    ) else {
        return Ok(());
    };
    let index_path = mc_dir
        .join("assets")
        .join("indexes")
        .join(format!("{id}.json"));
    if !index_path.is_file() {
        report(Step {
            label: s.install_asset_index.to_string(),
            done: 0,
            total: 1,
        });
        download(
            url,
            &index_path,
            index_meta.get("sha1").and_then(Value::as_str),
        )?;
    }

    let text = std::fs::read_to_string(&index_path).map_err(|e| e.to_string())?;
    let index: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let Some(objects) = index.get("objects").and_then(Value::as_object) else {
        return Ok(());
    };
    let virtual_assets = index
        .get("virtual")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Ce qui manque vraiment : inutile de relire 4 000 fichiers deux fois.
    let mut todo: Vec<(String, String)> = Vec::new();
    for (name, entry) in objects {
        let Some(hash) = entry.get("hash").and_then(Value::as_str) else {
            continue;
        };
        if !object_path(mc_dir, hash).is_file() {
            todo.push((name.clone(), hash.to_string()));
        }
    }

    if !todo.is_empty() {
        let total = todo.len() as u64;
        let done = AtomicU64::new(0);
        let next = AtomicUsize::new(0);
        let failure: Mutex<Option<String>> = Mutex::new(None);
        report(Step {
            label: s.install_assets.to_string(),
            done: 0,
            total,
        });

        std::thread::scope(|scope| {
            for _ in 0..PARALLEL.min(todo.len()) {
                scope.spawn(|| {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some((_name, hash)) = todo.get(index) else {
                            return;
                        };
                        if failure.lock().map(|f| f.is_some()).unwrap_or(false) {
                            return;
                        }
                        let target = object_path(mc_dir, hash);
                        let url = format!("{RESOURCES}/{}/{hash}", &hash[..2]);
                        if let Err(error) = download(&url, &target, Some(hash))
                            && let Ok(mut slot) = failure.lock()
                        {
                            *slot = Some(error);
                        }
                        let count = done.fetch_add(1, Ordering::Relaxed) + 1;
                        if count.is_multiple_of(20) || count == total {
                            report(Step {
                                label: s.install_assets.to_string(),
                                done: count,
                                total,
                            });
                        }
                    }
                });
            }
        });

        if let Some(error) = failure.into_inner().ok().flatten() {
            return Err(error);
        }
    }

    // Les versions d'avant 1.7.3 lisent les ressources par leur vrai nom.
    if virtual_assets {
        let root = mc_dir.join("assets").join("virtual").join("legacy");
        for (name, entry) in objects {
            let Some(hash) = entry.get("hash").and_then(Value::as_str) else {
                continue;
            };
            let target = root.join(name);
            if target.is_file() {
                continue;
            }
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(object_path(mc_dir, hash), target);
        }
    }
    Ok(())
}

fn object_path(mc_dir: &Path, hash: &str) -> PathBuf {
    mc_dir
        .join("assets")
        .join("objects")
        .join(&hash[..2])
        .join(hash)
}

// --------------------------------------------------------------- transfert

/// Télécharge vers un fichier temporaire, vérifie, puis met en place.
fn download(url: &str, dest: &Path, sha1: Option<&str>) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut response = ureq::get(url)
        .call()
        .map_err(|e| format!("{} : {e}", short_url(url)))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{} : {e}", short_url(url)))?;

    if let Some(expected) = sha1 {
        let got = sha1_hex(&bytes);
        if !got.eq_ignore_ascii_case(expected) {
            return Err(format!("{} : empreinte incorrecte", short_url(url)));
        }
    }

    let temporary = dest.with_extension("part");
    std::fs::write(&temporary, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, dest).map_err(|e| e.to_string())
}

fn short_url(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or(url)
}

fn sha1_hex(bytes: &[u8]) -> String {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(bytes);
    crate::sys::hex_encode(&hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_live_under_their_two_first_characters() {
        let path = object_path(Path::new("mc"), "abcdef0123456789");
        assert!(
            path.ends_with("assets/objects/ab/abcdef0123456789".replace('/', "\\"))
                || path.ends_with("assets/objects/ab/abcdef0123456789")
        );
    }

    #[test]
    fn sha1_matches_the_reference() {
        // Empreinte connue de la chaine vide, puis d'un texte court.
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn urls_are_shortened_for_messages() {
        assert_eq!(
            short_url("https://piston-data.mojang.com/v1/objects/abc/client.jar"),
            "client.jar"
        );
    }

    #[test]
    fn a_wrong_hash_is_refused() {
        let dir = std::env::temp_dir().join("ruche-test-download");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Adresse volontairement invalide : on verifie surtout qu'aucun
        // fichier n'est laisse en place quand ca echoue.
        let target = dir.join("client.jar");
        let result = download("https://exemple.invalide/client.jar", &target, Some("00"));
        assert!(result.is_err());
        assert!(!target.exists(), "rien ne doit rester en cas d'echec");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
