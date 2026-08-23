//! Installation d'une version absente, contre les vrais serveurs de Mojang.
//!
//! Ces tests écrivent dans le `.minecraft` de la machine et téléchargent pour
//! de bon : ils sont ignorés par défaut.
//!
//! ```text
//! cargo test --test real_install -- --ignored --nocapture
//! ```

use ruche::config::Settings;
use ruche::i18n::Lang;
use ruche::mc::manifest::Catalog;
use ruche::mc::{command, install, version};

/// Version de test : petite, et son index de ressources est déjà là si 1.12.2
/// est installée — on télécharge donc le json, le jar et les libraries, pas
/// trois cents mégaoctets de sons.
const TARGET: &str = "1.12";

fn catalog(settings: &Settings) -> Catalog {
    let cache = ruche::config::config_dir();
    let disk = Catalog::load(&settings.mc_dir, &cache);
    if disk.remote_known {
        return disk;
    }
    Catalog::refresh(&settings.mc_dir, &cache).expect("manifeste injoignable")
}

#[test]
#[ignore = "telecharge vraiment depuis les serveurs de Mojang"]
fn une_version_absente_sinstalle() {
    let settings = Settings::default();
    let catalog = catalog(&settings);
    assert!(
        catalog.entries.len() > 500,
        "le manifeste devrait lister des centaines de versions, pas {}",
        catalog.entries.len()
    );

    let entry = catalog.find(TARGET).expect("version absente du manifeste");
    println!(
        "{TARGET} : installee={} prete={} url={}",
        entry.installed,
        entry.ready,
        entry.url.is_some()
    );

    // On repart d'un dossier propre pour que le test ait quelque chose a faire.
    let version_dir = settings.mc_dir.join("versions").join(TARGET);
    if version_dir.exists() {
        std::fs::remove_dir_all(&version_dir).expect("dossier de version verrouille");
        println!("dossier {TARGET} retire, on reinstalle");
    }
    assert!(
        !install::is_complete(&settings.mc_dir, TARGET),
        "la version ne devrait plus etre complete"
    );

    let s = Lang::Fr.strings();
    let report = |step: install::Step| {
        if step.total > 1 && !step.done.is_multiple_of(200) && step.done != step.total {
            return;
        }
        println!("  {} {}/{}", step.label, step.done, step.total);
    };
    install::ensure(&settings.mc_dir, TARGET, &catalog, s, &report).expect("installation");

    // Le json, le jar et les libraries doivent etre la.
    let resolved = version::resolve(&settings.mc_dir, TARGET).expect("version illisible");
    let jar = resolved.jar.clone().expect("jar client absent");
    let size = std::fs::metadata(&jar).unwrap().len();
    println!("jar : {} ({size} octets)", jar.display());
    assert!(size > 1_000_000, "jar suspect : {size} octets");

    let account = ruche::config::Account::offline("AltTest");
    let opts = command::LaunchOptions::from_settings(&settings, &account, &resolved);
    let (cmd, missing) =
        command::build(&settings.mc_dir, &resolved, &account, &opts).expect("commande");
    assert!(
        missing.is_empty(),
        "il manque encore {} fichier(s), dont {:?}",
        missing.len(),
        missing.first().map(|m| m.path.file_name())
    );
    assert!(cmd.iter().any(|a| a == &resolved.main_class));
    println!("commande complete : {} arguments", cmd.len());

    assert!(
        install::is_complete(&settings.mc_dir, TARGET),
        "la version devrait etre consideree comme prete"
    );

    // Et le catalogue doit maintenant la voir installee.
    let after = Catalog::load(&settings.mc_dir, &ruche::config::config_dir());
    let entry = after.find(TARGET).unwrap();
    assert!(entry.installed && entry.ready);
}

#[test]
#[ignore = "telecharge un fichier de ressources"]
fn une_ressource_manquante_est_retelechargee() {
    let settings = Settings::default();
    let mc_dir = &settings.mc_dir;
    let installed = version::list_versions(mc_dir);
    let Some(id) = installed.iter().find(|v| install::is_complete(mc_dir, v)) else {
        eprintln!("aucune version complete sous la main");
        return;
    };
    let resolved = version::resolve(mc_dir, id).unwrap();
    let index_id = resolved.asset_index.clone();
    let index_path = mc_dir
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));
    let Ok(text) = std::fs::read_to_string(&index_path) else {
        eprintln!("pas d'index de ressources pour {id}");
        return;
    };
    let index: serde_json::Value = serde_json::from_str(&text).unwrap();

    // On choisit un petit objet present sur le disque, et on le supprime.
    let objects = index.get("objects").and_then(|o| o.as_object()).unwrap();
    let victim = objects
        .iter()
        .filter_map(|(name, entry)| {
            let hash = entry.get("hash")?.as_str()?;
            let size = entry.get("size")?.as_u64()?;
            let path = mc_dir
                .join("assets")
                .join("objects")
                .join(&hash[..2])
                .join(hash);
            (size < 20_000 && path.is_file()).then(|| (name.clone(), hash.to_string(), path))
        })
        .next();
    let Some((name, hash, path)) = victim else {
        eprintln!("aucune ressource locale a retirer");
        return;
    };
    let original = std::fs::read(&path).unwrap();
    println!("on retire {name} ({hash}, {} octets)", original.len());
    std::fs::remove_file(&path).unwrap();
    assert!(!path.is_file());

    let s = Lang::Fr.strings();
    install::ensure(mc_dir, id, &catalog(&settings), s, &|_| {}).expect("reparation");

    let restored = std::fs::read(&path).expect("la ressource n'a pas ete remise");
    assert_eq!(restored, original, "le contenu doit etre identique");
    println!("ressource remise en place, contenu identique");
}
