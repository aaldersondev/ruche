//! Test de bout en bout : ce test lance reellement Minecraft.
//!
//! Il n'a de sens que sur une machine qui a deja l'installation officielle,
//! donc il est marque `#[ignore]`. Pour le jouer :
//!
//! ```text
//! cargo test --test real_launch -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use ruche::config::{Account, Settings};
use ruche::mc::manifest::Catalog;
use ruche::mc::{command, version};
use ruche::queue::{Manager, State};
use ruche::sys;

/// Versions essayees, de la plus legere a la plus lourde.
const CANDIDATES: [&str; 4] = ["1.8.9", "1.12.2", "1.20.1", "1.21.8"];

fn base_settings() -> Settings {
    Settings {
        instances_dir: std::env::temp_dir().join("ruche-tests"),
        xmx_mb: 1024,
        xms_mb: 256,
        stagger_min_s: 5,
        stagger_max_s: 120,
        width: 800,
        height: 450,
        // Le test ne doit pas etre bloque par le garde-fou de la machine hote.
        ignore_ram_guard: true,
        ..Default::default()
    }
}

/// Catalogue complet : disque, ou reseau si le cache est vide.
fn alt_catalog(settings: &Settings) -> Catalog {
    let cache = ruche::config::config_dir();
    let disk = Catalog::load(&settings.mc_dir, &cache);
    if disk.remote_known {
        disk
    } else {
        Catalog::refresh(&settings.mc_dir, &cache).expect("manifeste injoignable")
    }
}

fn first_installed(settings: &Settings) -> Option<String> {
    let installed = version::list_versions(&settings.mc_dir);
    // De quoi viser une version precise, installee ou non : la file la
    // telechargera au besoin. RUCHE_TEST_VERSION=1.20.1
    if let Ok(wanted) = std::env::var("RUCHE_TEST_VERSION") {
        return Some(wanted);
    }
    CANDIDATES
        .iter()
        .find(|c| installed.iter().any(|v| v == *c))
        .map(|v| v.to_string())
}

#[test]
#[ignore = "lance vraiment le jeu ; demande une installation Minecraft"]
fn le_client_demarre_vraiment() {
    let settings = base_settings();
    let Some(version_id) = first_installed(&settings) else {
        eprintln!("aucune version de test installee, rien a verifier");
        return;
    };
    println!("version testee : {version_id}");

    let manager = Manager::new(|| {});
    // Sans catalogue, la file ne saurait pas ou telecharger une version absente.
    manager.set_catalog(std::sync::Arc::new(alt_catalog(&settings)));
    let account = Account::offline("AltTest");
    let id = manager.enqueue(account, version_id.clone(), settings);

    let deadline = Instant::now() + Duration::from_secs(180);
    let mut pid = None;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let list = manager.shared.instances.lock().unwrap();
        let instance = list.iter().find(|i| i.id == id).unwrap();
        if instance.state == State::Running {
            pid = instance.pid;
            break;
        }
        assert!(
            !instance.state.is_over(),
            "le client s'est arrete tout de suite ({:?}, log {:?})",
            instance.state,
            instance.log_path
        );
    }

    let pid = pid.expect("le client n'a pas atteint l'etat « en jeu »");
    let rss = sys::process_rss_mb(pid);
    println!("pid {pid}, {rss} Mo en memoire");
    assert!(rss > 200, "le process ne consomme que {rss} Mo");

    manager.kill(id);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let list = manager.shared.instances.lock().unwrap();
        let instance = list.iter().find(|i| i.id == id).unwrap();
        if instance.state.is_over() {
            manager.shutdown();
            return;
        }
    }
    panic!("le client n'a pas ete arrete");
}

#[test]
#[ignore = "demande une installation Minecraft"]
fn chaque_version_installee_produit_une_commande_complete() {
    let settings = base_settings();
    let installed = version::list_versions(&settings.mc_dir);
    if installed.is_empty() {
        eprintln!("aucune version installee");
        return;
    }
    let account = Account::offline("AltTest");
    let mut checked = 0;
    for id in &installed {
        let Ok(resolved) = version::resolve(&settings.mc_dir, id) else {
            continue; // profil illisible : ce n'est pas l'objet du test
        };
        if resolved.jar.is_none() {
            println!("{id} : jar client jamais telecharge, ignore");
            continue;
        }
        let opts = command::LaunchOptions::from_settings(&settings, &account, &resolved);
        let (cmd, missing) = command::build(&settings.mc_dir, &resolved, &account, &opts)
            .unwrap_or_else(|e| panic!("{id} : {e}"));

        assert!(
            cmd.iter().any(|a| a == &resolved.main_class),
            "{id} : la classe principale manque"
        );
        assert!(
            cmd.iter().any(|a| a.starts_with("-Xmx")),
            "{id} : pas de plafond memoire"
        );
        assert!(
            cmd.iter().any(|a| a == "--username"),
            "{id} : pas de pseudo"
        );
        // Une library absente n'est pas bloquante tant que le json donne une
        // URL : la file la telecharge avant de lancer.
        let orphans: Vec<_> = missing.iter().filter(|m| m.url.is_none()).collect();
        assert!(
            orphans.is_empty(),
            "{id} : {} fichier(s) introuvables et sans URL, dont {:?}",
            orphans.len(),
            orphans.first().map(|m| m.path.file_name())
        );
        checked += 1;
        match missing.len() {
            0 => println!("{id} : {} arguments, classpath complet", cmd.len()),
            n => println!(
                "{id} : {} arguments, {n} library(ies) a telecharger au lancement",
                cmd.len()
            ),
        }
    }
    assert!(checked > 0, "aucune version exploitable");
}

/// Le cas vu en usage reel : une instance retenue par le plafond doit le dire,
/// au lieu d'annoncer un manque de memoire.
#[test]
#[ignore = "lance vraiment un client"]
fn le_plafond_dinstances_est_nomme() {
    let mut settings = base_settings();
    settings.max_instances = 1;
    settings.ignore_ram_guard = true; // on veut isoler le plafond, pas la RAM
    settings.wait_timeout_s = 120;
    let Some(version_id) = first_installed(&settings) else {
        eprintln!("aucune version de test installee");
        return;
    };

    let manager = Manager::new(|| {});
    manager.set_catalog(std::sync::Arc::new(alt_catalog(&settings)));
    let premier = manager.enqueue(
        Account::offline("AltCap1"),
        version_id.clone(),
        settings.clone(),
    );
    let second = manager.enqueue(Account::offline("AltCap2"), version_id, settings);

    // On attend que le premier tourne et que le second soit retenu.
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut etat = State::Queued;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let list = manager.shared.instances.lock().unwrap();
        etat = list.iter().find(|i| i.id == second).unwrap().state;
        if etat == State::WaitingSlot {
            break;
        }
        assert!(
            !etat.is_over(),
            "le second n'aurait pas du s'arreter ({etat:?})"
        );
    }
    assert_eq!(
        etat,
        State::WaitingSlot,
        "le second doit annoncer le plafond, pas la memoire"
    );

    let log = manager.shared.log.lock().unwrap();
    assert!(
        log.iter().any(|l| l.contains("plafond")),
        "le journal doit dire la meme chose que la carte : {log:?}"
    );
    drop(log);
    println!(
        "second en attente : {}",
        etat.label(ruche::i18n::Lang::Fr.strings())
    );

    manager.clear_queue();
    manager.kill(premier);
    std::thread::sleep(Duration::from_secs(3));
    manager.shutdown();
}
