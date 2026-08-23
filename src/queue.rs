//! File de lancement : c'est ici qu'on empeche la machine de tomber.
//!
//! Une seule instance demarre a la fois, et aucune ne part s'il ne reste pas
//! assez de RAM physique une fois la reserve systeme mise de cote.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::auth::msa;
use crate::config::{Account, Settings};
use crate::i18n::{Lang, Strings, fill};
use crate::mc::command::{self, LaunchOptions};
use crate::mc::version::{self, Version};
use crate::sys;

const LOG_CAP: usize = 400;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Queued,
    WaitingRoom,
    Starting,
    Running,
    Stopped,
    Crashed,
    Aborted,
}

impl State {
    pub fn label(self, s: &'static Strings) -> &'static str {
        match self {
            State::Queued => s.state_queued,
            State::WaitingRoom => s.state_waiting,
            State::Starting => s.state_starting,
            State::Running => s.state_running,
            State::Stopped => s.state_stopped,
            State::Crashed => s.state_crashed,
            State::Aborted => s.state_aborted,
        }
    }

    pub fn is_pending(self) -> bool {
        matches!(self, State::Queued | State::WaitingRoom | State::Starting)
    }

    pub fn is_over(self) -> bool {
        matches!(self, State::Stopped | State::Crashed | State::Aborted)
    }
}

pub struct Instance {
    pub id: u64,
    pub account: String,
    pub version: String,
    pub state: State,
    pub pid: Option<u32>,
    pub rss_mb: u64,
    pub started_at: Option<Instant>,
    /// Meme instant, en secondes epoch : Discord veut un horodatage absolu.
    pub started_epoch: Option<u64>,
    pub exit_code: Option<i32>,
    pub log_path: Option<PathBuf>,
    child: Option<Child>,
    cancelled: bool,
}

impl Instance {
    pub fn uptime(&self) -> String {
        match self.started_at {
            None => String::new(),
            Some(start) => {
                let secs = start.elapsed().as_secs();
                format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
            }
        }
    }
}

struct Job {
    id: u64,
    account: Account,
    version: String,
    settings: Settings,
}

pub struct Shared {
    /// Langue courante : le journal est ecrit dedans, y compris depuis les threads.
    lang: Mutex<Lang>,
    pub instances: Mutex<Vec<Instance>>,
    pub log: Mutex<VecDeque<String>>,
    /// Comptes premium dont la session a ete renouvelee, a reporter dans l'UI.
    pub refreshed: Mutex<Vec<Account>>,
    notify: Box<dyn Fn() + Send + Sync>,
    stopping: AtomicBool,
}

impl Shared {
    pub fn s(&self) -> &'static Strings {
        self.lang
            .lock()
            .map(|lang| lang.strings())
            .unwrap_or_else(|_| Lang::default().strings())
    }

    pub fn running_count(&self) -> usize {
        self.instances
            .lock()
            .map(|list| {
                list.iter()
                    .filter(|i| matches!(i.state, State::Running | State::Starting))
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn pending_count(&self) -> usize {
        self.instances
            .lock()
            .map(|list| list.iter().filter(|i| i.state.is_pending()).count())
            .unwrap_or(0)
    }

    fn log(&self, message: impl Into<String>) {
        if let Ok(mut log) = self.log.lock() {
            let stamped = format!("{}  {}", sys::clock(), message.into());
            log.push_back(stamped);
            while log.len() > LOG_CAP {
                log.pop_front();
            }
        }
        (self.notify)();
    }

    fn update(&self, id: u64, apply: impl FnOnce(&mut Instance)) {
        if let Ok(mut list) = self.instances.lock()
            && let Some(instance) = list.iter_mut().find(|i| i.id == id)
        {
            apply(instance);
        }
        (self.notify)();
    }

    fn is_cancelled(&self, id: u64) -> bool {
        self.instances
            .lock()
            .map(|list| {
                list.iter()
                    .find(|i| i.id == id)
                    .map(|i| i.cancelled)
                    .unwrap_or(true)
            })
            .unwrap_or(true)
    }
}

pub struct Manager {
    pub shared: Arc<Shared>,
    sender: mpsc::Sender<Job>,
    next_id: AtomicU64,
}

impl Manager {
    /// `notify` est appele des qu'il y a du neuf a afficher.
    pub fn new(notify: impl Fn() + Send + Sync + 'static) -> Self {
        let shared = Arc::new(Shared {
            lang: Mutex::new(Lang::default()),
            instances: Mutex::new(Vec::new()),
            log: Mutex::new(VecDeque::new()),
            refreshed: Mutex::new(Vec::new()),
            notify: Box::new(notify),
            stopping: AtomicBool::new(false),
        });
        let (sender, receiver) = mpsc::channel::<Job>();

        let worker_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("file-de-lancement".into())
            .spawn(move || worker(worker_shared, receiver))
            .expect("thread de lancement");

        let watch_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("surveillance".into())
            .spawn(move || watcher(watch_shared))
            .expect("thread de surveillance");

        Self {
            shared,
            sender,
            next_id: AtomicU64::new(1),
        }
    }

    pub fn log(&self, message: impl Into<String>) {
        self.shared.log(message);
    }

    /// Change la langue du journal (les threads la relisent a chaque message).
    pub fn set_lang(&self, lang: Lang) {
        if let Ok(mut slot) = self.shared.lang.lock() {
            *slot = lang;
        }
    }

    pub fn strings(&self) -> &'static Strings {
        self.shared.s()
    }

    pub fn enqueue(&self, account: Account, version: String, settings: Settings) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut list) = self.shared.instances.lock() {
            list.push(Instance {
                id,
                account: account.name.clone(),
                version: version.clone(),
                state: State::Queued,
                pid: None,
                rss_mb: 0,
                started_at: None,
                started_epoch: None,
                exit_code: None,
                log_path: None,
                child: None,
                cancelled: false,
            });
        }
        self.shared
            .log(fill(self.shared.s().log_queued, &[&account.name, &version]));
        let _ = self.sender.send(Job {
            id,
            account,
            version,
            settings,
        });
        id
    }

    /// Ferme une instance, ou la retire de la file si elle n'a pas demarre.
    pub fn kill(&self, id: u64) {
        let mut name = String::new();
        if let Ok(mut list) = self.shared.instances.lock()
            && let Some(instance) = list.iter_mut().find(|i| i.id == id)
        {
            name = instance.account.clone();
            instance.cancelled = true;
            match instance.child.as_mut() {
                Some(child) => {
                    let _ = child.kill();
                }
                None => instance.state = State::Aborted,
            }
        }
        if !name.is_empty() {
            self.shared
                .log(fill(self.shared.s().log_stop_requested, &[&name]));
        }
    }

    pub fn kill_all(&self) {
        let ids: Vec<u64> = self
            .shared
            .instances
            .lock()
            .map(|list| {
                list.iter()
                    .filter(|i| !i.state.is_over())
                    .map(|i| i.id)
                    .collect()
            })
            .unwrap_or_default();
        for id in ids {
            self.kill(id);
        }
    }

    /// Vide la file d'attente sans toucher aux instances deja lancees.
    pub fn clear_queue(&self) {
        let mut count = 0;
        if let Ok(mut list) = self.shared.instances.lock() {
            for instance in list
                .iter_mut()
                .filter(|i| matches!(i.state, State::Queued | State::WaitingRoom) && !i.cancelled)
            {
                instance.cancelled = true;
                instance.state = State::Aborted;
                count += 1;
            }
        }
        if count > 0 {
            self.shared.log(fill(
                self.shared.s().log_queue_cleared,
                &[&count.to_string()],
            ));
        }
    }

    /// Retire les lignes terminees du tableau.
    pub fn forget_finished(&self) {
        if let Ok(mut list) = self.shared.instances.lock() {
            list.retain(|i| !i.state.is_over());
        }
        (self.shared.notify)();
    }

    /// Comptes premium rafraichis depuis le dernier appel.
    pub fn take_refreshed(&self) -> Vec<Account> {
        self.shared
            .refreshed
            .lock()
            .map(|mut list| std::mem::take(&mut *list))
            .unwrap_or_default()
    }

    pub fn shutdown(&self) {
        self.shared.stopping.store(true, Ordering::Relaxed);
    }
}

fn worker(shared: Arc<Shared>, receiver: mpsc::Receiver<Job>) {
    let mut cache: HashMap<(PathBuf, String), Version> = HashMap::new();
    while let Ok(job) = receiver.recv() {
        if shared.stopping.load(Ordering::Relaxed) || shared.is_cancelled(job.id) {
            continue;
        }
        if !wait_for_room(&shared, &job) {
            continue;
        }
        if let Err(message) = start(&shared, &job, &mut cache) {
            shared.update(job.id, |i| i.state = State::Crashed);
            shared.log(format!("[{}] {message}", job.account.name));
        }
    }
}

/// Bloque tant qu'il n'y a pas la place pour une instance de plus.
fn wait_for_room(shared: &Arc<Shared>, job: &Job) -> bool {
    let settings = &job.settings;
    let xmx = job.account.xmx_mb.unwrap_or(settings.xmx_mb);
    let needed = xmx + settings.overhead_mb;
    let mut waited = 0u64;
    let mut announced = false;

    loop {
        if shared.stopping.load(Ordering::Relaxed) || shared.is_cancelled(job.id) {
            return false;
        }
        let slot_free = shared.running_count() < settings.max_instances.max(1);
        let (_total, avail) = sys::memory_mb();
        let ram_ok =
            settings.ignore_ram_guard || avail.saturating_sub(needed) >= settings.reserve_mb;
        if slot_free && ram_ok {
            return true;
        }
        if !announced {
            announced = true;
            shared.update(job.id, |i| i.state = State::WaitingRoom);
            let s = shared.s();
            if !slot_free {
                shared.log(fill(
                    s.log_cap_reached,
                    &[&job.account.name, &settings.max_instances.to_string()],
                ));
            } else {
                shared.log(fill(
                    s.log_ram_short,
                    &[
                        &job.account.name,
                        &avail.to_string(),
                        &needed.to_string(),
                        &settings.reserve_mb.to_string(),
                    ],
                ));
            }
        }
        std::thread::sleep(Duration::from_secs(3));
        waited += 3;
        if waited >= settings.wait_timeout_s {
            shared.update(job.id, |i| i.state = State::Aborted);
            shared.log(fill(
                shared.s().log_gave_up,
                &[&job.account.name, &waited.to_string()],
            ));
            return false;
        }
    }
}

fn start(
    shared: &Arc<Shared>,
    job: &Job,
    cache: &mut HashMap<(PathBuf, String), Version>,
) -> Result<(), String> {
    let settings = &job.settings;
    let mut account = job.account.clone();

    // Compte premium : on renouvelle la session avant de lancer.
    match msa::ensure_valid(
        &mut account,
        &settings.azure_client_id,
        settings.lang,
        |m| shared.log(m),
    ) {
        Ok(true) => {
            if let Ok(mut list) = shared.refreshed.lock() {
                list.push(account.clone());
            }
        }
        Ok(false) => {}
        Err(e) => return Err(fill(shared.s().log_premium_error, &[&e.0])),
    }

    let key = (settings.mc_dir.clone(), job.version.clone());
    if !cache.contains_key(&key) {
        let resolved = version::resolve(&settings.mc_dir, &job.version)?;
        cache.insert(key.clone(), resolved);
    }
    let version = &cache[&key];

    let opts = LaunchOptions::from_settings(settings, &account, version);
    std::fs::create_dir_all(&opts.game_dir).map_err(|e| format!("dossier d'instance : {e}"))?;
    let _ = command::seed_options(&opts.game_dir, settings.low_settings);
    if settings.add_server_entry
        && let Some((host, port)) = settings.server_host_port()
    {
        let _ = command::write_servers_dat(&opts.game_dir, &host, port, &settings.server_name);
    }
    if settings.share_mods {
        for name in ["mods", "resourcepacks", "shaderpacks", "config"] {
            let src = settings.mc_dir.join(name);
            if src.is_dir() {
                command::link_dir(&src, &opts.game_dir.join(name));
            }
        }
    }

    let (mut cmd, missing) = command::build(&settings.mc_dir, version, &account, &opts)?;
    if !missing.is_empty() {
        shared.log(fill(
            shared.s().log_missing_files,
            &[&account.name, &missing.len().to_string()],
        ));
        let failed = command::download_missing(&missing, shared.s(), |m| shared.log(m));
        if !failed.is_empty() {
            let names: Vec<String> = failed
                .iter()
                .take(3)
                .map(|p| {
                    p.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                })
                .collect();
            return Err(fill(shared.s().log_missing_failed, &[&names.join(", ")]));
        }
        cmd = command::build(&settings.mc_dir, version, &account, &opts)?.0;
    }

    // Le log garde la commande complete en premiere ligne : elle est rejouable.
    let logs_dir = opts.game_dir.join("logs");
    std::fs::create_dir_all(&logs_dir).map_err(|e| format!("dossier de logs : {e}"))?;
    let log_path = logs_dir.join(format!("ruche-{}.log", sys::file_stamp()));
    let mut log_file =
        std::fs::File::create(&log_path).map_err(|e| format!("fichier de log : {e}"))?;
    {
        use std::io::Write;
        let quoted: Vec<String> = cmd
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect();
        let _ = writeln!(log_file, "{}\n", quoted.join(" "));
    }
    let stderr_file = log_file
        .try_clone()
        .map_err(|e| format!("duplication du log : {e}"))?;

    shared.log(fill(
        shared.s().log_launching,
        &[
            &account.name,
            &job.version,
            &opts.xmx_mb.to_string(),
            &opts.java.file_name().unwrap_or_default().to_string_lossy(),
        ],
    ));

    let mut process = Command::new(&cmd[0]);
    process
        .args(&cmd[1..])
        .current_dir(&opts.game_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(Stdio::from(stderr_file));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        process.creation_flags(CREATE_NO_WINDOW | settings.priority.creation_flag());
    }
    let child = process
        .spawn()
        .map_err(|e| fill(shared.s().log_java_failed, &[&e.to_string()]))?;
    let pid = child.id();
    let slot = shared.running_count();

    shared.update(job.id, |instance| {
        instance.child = Some(child);
        instance.pid = Some(pid);
        instance.state = State::Starting;
        instance.started_at = Some(Instant::now());
        instance.started_epoch = Some(crate::config::now_secs());
        instance.log_path = Some(log_path.clone());
    });

    if let Some(mask) = sys::affinity_mask(slot, settings.cores_per_instance) {
        sys::set_affinity(pid, mask);
    }

    wait_until_loaded(shared, job, pid);
    Ok(())
}

/// Retient la file tant que le client n'a pas fini de charger.
fn wait_until_loaded(shared: &Arc<Shared>, job: &Job, pid: u32) {
    let settings = &job.settings;
    let xmx = job.account.xmx_mb.unwrap_or(settings.xmx_mb);
    let target_rss = (xmx * 35 / 100).max(400);
    let floor = Instant::now() + Duration::from_secs(settings.stagger_min_s);
    let deadline = Instant::now() + Duration::from_secs(settings.stagger_max_s.max(1));

    while Instant::now() < deadline {
        if shared.stopping.load(Ordering::Relaxed) {
            return;
        }
        let alive = shared
            .instances
            .lock()
            .map(|list| {
                list.iter()
                    .find(|i| i.id == job.id)
                    .map(|i| !i.state.is_over())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !alive {
            return;
        }
        if Instant::now() >= floor {
            let rss = sys::process_rss_mb(pid);
            shared.update(job.id, |i| i.rss_mb = rss);
            if sys::has_visible_window(pid) || rss >= target_rss {
                shared.update(job.id, |i| {
                    if i.state == State::Starting {
                        i.state = State::Running;
                    }
                });
                shared.log(fill(
                    shared.s().log_loaded,
                    &[&job.account.name, &pid.to_string()],
                ));
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    shared.update(job.id, |i| {
        if i.state == State::Starting {
            i.state = State::Running;
        }
    });
    shared.log(fill(
        shared.s().log_still_loading,
        &[&job.account.name, &settings.stagger_max_s.to_string()],
    ));
}

/// Suit la memoire des clients et ramasse ceux qui se terminent.
fn watcher(shared: Arc<Shared>) {
    loop {
        std::thread::sleep(Duration::from_secs(2));
        if shared.stopping.load(Ordering::Relaxed) {
            return;
        }
        let mut messages = Vec::new();
        let mut changed = false;
        if let Ok(mut list) = shared.instances.lock() {
            for instance in list.iter_mut() {
                let Some(child) = instance.child.as_mut() else {
                    continue;
                };
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code().unwrap_or(-1);
                        instance.exit_code = Some(code);
                        instance.state = if instance.cancelled || code == 0 || code == 1 {
                            State::Stopped
                        } else {
                            State::Crashed
                        };
                        instance.rss_mb = 0;
                        instance.child = None;
                        let s = shared.s();
                        let detail = match instance.log_path.as_ref() {
                            Some(path) if instance.state == State::Crashed => {
                                fill(s.log_see, &[&path.display().to_string()])
                            }
                            _ => String::new(),
                        };
                        messages.push(fill(
                            s.log_finished,
                            &[&instance.account, &code.to_string(), &detail],
                        ));
                        changed = true;
                    }
                    Ok(None) => {
                        if let Some(pid) = instance.pid {
                            let rss = sys::process_rss_mb(pid);
                            if rss != instance.rss_mb {
                                instance.rss_mb = rss;
                                changed = true;
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        for message in messages {
            shared.log(message);
        }
        if changed {
            (shared.notify)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_are_classified() {
        assert!(State::Queued.is_pending());
        assert!(State::WaitingRoom.is_pending());
        assert!(State::Starting.is_pending());
        assert!(!State::Running.is_pending());
        assert!(State::Crashed.is_over());
        assert!(State::Aborted.is_over());
        assert!(!State::Running.is_over());
    }

    #[test]
    fn queue_can_be_emptied_before_anything_starts() {
        let manager = Manager::new(|| {});
        let settings = Settings {
            // reserve absurde : la file ne lancera jamais rien pendant le test
            reserve_mb: u64::MAX / 2,
            wait_timeout_s: 3600,
            ..Default::default()
        };
        let id = manager.enqueue(Account::offline("Alt1"), "1.8.9".into(), settings);
        std::thread::sleep(Duration::from_millis(200));
        manager.clear_queue();
        let list = manager.shared.instances.lock().unwrap();
        let instance = list.iter().find(|i| i.id == id).unwrap();
        assert_eq!(instance.state, State::Aborted);
        assert!(instance.pid.is_none(), "aucun process ne devait démarrer");
        manager.shutdown();
    }

    #[test]
    fn the_guard_refuses_to_launch_without_memory() {
        let manager = Manager::new(|| {});
        let settings = Settings {
            reserve_mb: u64::MAX / 2, // jamais satisfaisable
            wait_timeout_s: 3,        // abandon immediat
            ..Default::default()
        };
        let id = manager.enqueue(Account::offline("Alt2"), "1.8.9".into(), settings);
        std::thread::sleep(Duration::from_secs(7));
        let list = manager.shared.instances.lock().unwrap();
        let instance = list.iter().find(|i| i.id == id).unwrap();
        assert_eq!(instance.state, State::Aborted);
        assert!(instance.pid.is_none());
        drop(list);
        let log = manager.shared.log.lock().unwrap();
        assert!(
            log.iter().any(|line| line.contains("RAM insuffisante")),
            "le journal doit expliquer le refus : {log:?}"
        );
        manager.shutdown();
    }

    #[test]
    fn uptime_is_formatted() {
        let instance = Instance {
            id: 1,
            account: "Alt".into(),
            version: "1.8.9".into(),
            state: State::Running,
            pid: None,
            rss_mb: 0,
            started_at: None,
            started_epoch: None,
            exit_code: None,
            log_path: None,
            child: None,
            cancelled: false,
        };
        assert_eq!(instance.uptime(), "");
    }
}
