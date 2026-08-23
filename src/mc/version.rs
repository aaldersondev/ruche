//! Lecture des versions installees dans `.minecraft/versions`.
//!
//! Un profil OptiFine, Fabric ou Forge ne contient qu'un fragment de json et
//! pointe vers son parent par `inheritsFrom` : tout est fusionne ici pour ne
//! manipuler ensuite qu'une seule description complete.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Etat des « features » sur lesquelles les regles des json peuvent porter.
#[derive(Clone, Copy, Default)]
pub struct Features {
    pub demo: bool,
    pub custom_resolution: bool,
    pub quick_play_multiplayer: bool,
}

impl Features {
    pub fn get(&self, name: &str) -> bool {
        match name {
            "is_demo_user" => self.demo,
            "has_custom_resolution" => self.custom_resolution,
            "has_quick_plays_support" | "is_quick_play_multiplayer" => self.quick_play_multiplayer,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Version {
    /// Identifiant choisi par l'utilisateur (celui du dossier).
    pub id: String,
    /// Du plus specifique (l'id) au plus generique (le vanilla).
    pub chain: Vec<String>,
    /// Version vanilla en bout de chaine : JRE, natives, quickPlay.
    pub root: String,
    /// Jar client : celui du dossier choisi si l'installeur l'y a recopie.
    pub jar: Option<PathBuf>,
    pub main_class: String,
    pub asset_index: String,
    pub assets: String,
    pub version_type: String,
    pub java_component: Option<String>,
    pub java_major: u32,
    /// Json fusionne, pour les libraries et les arguments.
    pub raw: Value,
}

impl Version {
    pub fn libraries(&self) -> &[Value] {
        self.raw
            .get("libraries")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn arguments(&self, section: &str) -> Option<&Vec<Value>> {
        self.raw.get("arguments")?.get(section)?.as_array()
    }

    pub fn legacy_arguments(&self) -> Option<&str> {
        self.raw.get("minecraftArguments").and_then(Value::as_str)
    }

    /// `--quickPlayMultiplayer` n'existe qu'a partir de 1.20.
    pub fn supports_quick_play(&self) -> bool {
        match parse_release(&self.root) {
            Some((1, minor, _)) => minor >= 20,
            Some((major, _, _)) => major >= 20,
            None => false,
        }
    }

    /// Chemin des natives partagees pour cette version.
    pub fn natives_dir(&self, mc_dir: &Path) -> PathBuf {
        mc_dir.join("natives-ruche").join(&self.root)
    }
}

/// Toutes les versions installees, les plus recentes en tete.
pub fn list_versions(mc_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(mc_dir.join("versions")) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if entry.path().join(format!("{name}.json")).is_file() {
            out.push(name);
        }
    }
    out.sort_by_key(|name| sort_key(name));
    out
}

/// Trie 1.21.8 avant 1.8.9, et les noms non numeriques a la fin.
pub(crate) fn sort_key(name: &str) -> (u8, i64, i64, i64, String) {
    match parse_release(name) {
        Some((a, b, c)) => (
            0,
            -(a as i64),
            -(b as i64),
            -(c as i64),
            name.to_lowercase(),
        ),
        None => (1, 0, 0, 0, name.to_lowercase()),
    }
}

/// Extrait `1.20.1` d'un identifiant de version quelconque.
fn parse_release(id: &str) -> Option<(u32, u32, u32)> {
    let mut it = id.split(|c: char| !c.is_ascii_digit());
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next().unwrap_or("0").parse().unwrap_or(0);
    let patch: u32 = it.next().unwrap_or("0").parse().unwrap_or(0);
    Some((major, minor, patch))
}

/// Charge une version et resout toute sa chaine d'heritage.
pub fn resolve(mc_dir: &Path, id: &str) -> Result<Version, String> {
    let mut chain = Vec::new();
    let merged = load_chain(mc_dir, id, &mut chain, 0)?;

    // Le jar client est celui de la version choisie quand l'installeur l'a
    // recopie (Forge l'exige : son ignoreList ne couvre que son propre nom).
    let jar = chain
        .iter()
        .map(|v| mc_dir.join("versions").join(v).join(format!("{v}.jar")))
        .find(|p| p.is_file());

    let root = chain.last().cloned().unwrap_or_else(|| id.to_string());
    let java_component = merged
        .get("javaVersion")
        .and_then(|j| j.get("component"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let java_major = merged
        .get("javaVersion")
        .and_then(|j| j.get("majorVersion"))
        .and_then(Value::as_u64)
        .map(|v| v as u32)
        .or_else(|| java_component.as_deref().and_then(component_major))
        .unwrap_or_else(|| infer_java_major(&root));

    let asset_index = merged
        .get("assetIndex")
        .and_then(|a| a.get("id"))
        .and_then(Value::as_str)
        .or_else(|| merged.get("assets").and_then(Value::as_str))
        .unwrap_or("legacy")
        .to_string();

    Ok(Version {
        id: id.to_string(),
        root,
        jar,
        main_class: merged
            .get("mainClass")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{id} : mainClass absent du json"))?
            .to_string(),
        assets: merged
            .get("assets")
            .and_then(Value::as_str)
            .unwrap_or(&asset_index)
            .to_string(),
        asset_index,
        version_type: merged
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("release")
            .to_string(),
        java_component,
        java_major,
        chain,
        raw: merged,
    })
}

fn load_chain(
    mc_dir: &Path,
    id: &str,
    chain: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    if depth > 8 {
        return Err(format!("heritage trop profond a partir de {id}"));
    }
    if chain.iter().any(|v| v == id) {
        return Err(format!("boucle inheritsFrom sur {id}"));
    }
    let path = mc_dir.join("versions").join(id).join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{} illisible : {e}", path.display()))?;
    let mut data: Value =
        serde_json::from_str(&text).map_err(|e| format!("{id}.json invalide : {e}"))?;
    chain.push(id.to_string());

    if let Some(parent_id) = data
        .get("inheritsFrom")
        .and_then(Value::as_str)
        .map(str::to_string)
    {
        let parent = load_chain(mc_dir, &parent_id, chain, depth + 1)?;
        data = merge(parent, data);
    }
    Ok(data)
}

/// Fusionne un fragment (Forge, Fabric, OptiFine) au-dessus de son parent.
fn merge(parent: Value, child: Value) -> Value {
    let (Value::Object(mut out), Value::Object(child_map)) = (parent, child) else {
        return Value::Null;
    };
    for (key, value) in child_map {
        match key.as_str() {
            "inheritsFrom" => {}
            // Les libraries de l'enfant passent devant : elles surchargent
            // celles du parent lors du dedoublonnage du classpath.
            "libraries" => {
                let mut libs = value.as_array().cloned().unwrap_or_default();
                if let Some(existing) = out.get("libraries").and_then(Value::as_array) {
                    libs.extend(existing.iter().cloned());
                }
                out.insert(key, Value::Array(libs));
            }
            // Les arguments s'ajoutent a ceux du parent, dans cet ordre.
            "arguments" => {
                let mut merged = out
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                for section in ["game", "jvm"] {
                    let mut items = merged
                        .get(section)
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if let Some(extra) = value.get(section).and_then(Value::as_array) {
                        items.extend(extra.iter().cloned());
                    }
                    if !items.is_empty() {
                        merged.insert(section.to_string(), Value::Array(items));
                    }
                }
                out.insert(key, Value::Object(merged));
            }
            _ => {
                out.insert(key, value);
            }
        }
    }
    Value::Object(out)
}

pub fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// Evalue un bloc `rules` (ou `compatibilityRules`) de json de version.
pub fn rules_allow(rules: Option<&Value>, features: &Features) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else {
        return true;
    };
    let mut allowed = false;
    for rule in rules {
        let mut ok = true;
        if let Some(os) = rule.get("os") {
            if let Some(name) = os.get("name").and_then(Value::as_str)
                && name != os_name()
            {
                ok = false;
            }
            if let Some(arch) = os.get("arch").and_then(Value::as_str)
                && !matches!(arch, "x64" | "x86_64")
            {
                ok = false;
            }
        }
        if ok && let Some(feats) = rule.get("features").and_then(Value::as_object) {
            for (name, wanted) in feats {
                if features.get(name) != wanted.as_bool().unwrap_or(false) {
                    ok = false;
                    break;
                }
            }
        }
        if ok {
            allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

/// Les regles d'une library, quel que soit le nom de la cle.
pub fn lib_rules(lib: &Value) -> Option<&Value> {
    lib.get("rules").or_else(|| lib.get("compatibilityRules"))
}

fn component_major(component: &str) -> Option<u32> {
    Some(match component {
        "jre-legacy" => 8,
        "java-runtime-alpha" => 16,
        "java-runtime-beta" | "java-runtime-gamma" | "java-runtime-gamma-snapshot" => 17,
        "java-runtime-delta" => 21,
        "java-runtime-epsilon" => 25,
        _ => return None,
    })
}

/// Quand le json ne dit rien (certains profils tiers), on deduit du numero.
pub fn infer_java_major(root: &str) -> u32 {
    match parse_release(root) {
        Some((1, minor, patch)) => match minor {
            0..=16 => 8,
            17 => 16,
            18 | 19 => 17,
            20 if patch < 5 => 17,
            _ => 21,
        },
        Some((major, _, _)) if major >= 20 => 25,
        _ => 21,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn newest_versions_come_first() {
        let mut names = vec![
            "1.8.9".to_string(),
            "26.2".to_string(),
            "1.21.8".to_string(),
            "BatMod".to_string(),
            "1.12.2".to_string(),
        ];
        names.sort_by_key(|name| sort_key(name));
        assert_eq!(names, ["26.2", "1.21.8", "1.12.2", "1.8.9", "BatMod"]);
    }

    #[test]
    fn java_version_is_inferred_from_the_number() {
        assert_eq!(infer_java_major("1.8.9"), 8);
        assert_eq!(infer_java_major("1.16.5"), 8);
        assert_eq!(infer_java_major("1.17.1"), 16);
        assert_eq!(infer_java_major("1.20.1"), 17);
        assert_eq!(infer_java_major("1.20.6"), 21);
        assert_eq!(infer_java_major("1.21.8"), 21);
        assert_eq!(infer_java_major("26.2"), 25);
    }

    #[test]
    fn child_libraries_and_arguments_win() {
        let parent = json!({
            "mainClass": "net.minecraft.client.main.Main",
            "libraries": [{"name": "vanilla:lib:1"}],
            "arguments": {"game": ["--username", "${auth_player_name}"], "jvm": ["-cp"]},
            "type": "release"
        });
        let child = json!({
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [{"name": "forge:lib:2"}],
            "arguments": {"game": ["--launchTarget"], "jvm": ["-p"]}
        });
        let merged = merge(parent, child);
        assert_eq!(
            merged["mainClass"],
            "cpw.mods.bootstraplauncher.BootstrapLauncher"
        );
        assert_eq!(merged["libraries"][0]["name"], "forge:lib:2");
        assert_eq!(merged["libraries"][1]["name"], "vanilla:lib:1");
        assert_eq!(merged["arguments"]["game"][2], "--launchTarget");
        assert_eq!(merged["arguments"]["jvm"][1], "-p");
        assert_eq!(merged["type"], "release");
    }

    #[test]
    fn os_rules_are_respected() {
        let f = Features::default();
        let only_osx = json!([{"action": "allow", "os": {"name": "osx"}}]);
        assert_eq!(rules_allow(Some(&only_osx), &f), os_name() == "osx");

        let all_but_osx = json!([
            {"action": "allow"},
            {"action": "disallow", "os": {"name": "osx"}}
        ]);
        assert_eq!(rules_allow(Some(&all_but_osx), &f), os_name() != "osx");
        assert!(rules_allow(None, &f));
    }

    #[test]
    fn feature_rules_follow_the_launch_options() {
        let rule = json!([{
            "action": "allow",
            "features": {"has_custom_resolution": true}
        }]);
        let mut f = Features::default();
        assert!(!rules_allow(Some(&rule), &f));
        f.custom_resolution = true;
        assert!(rules_allow(Some(&rule), &f));
    }
}
