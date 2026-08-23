//! Interface : barre de navigation, bandeau de lancement, onglets et journal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui::{Color32, CornerRadius, Margin, RichText, Stroke, Vec2};

use crate::auth::msa::{self, DeviceFlow};
use crate::config::{Account, Priority, Settings, load_accounts, sanitize, save_accounts};
use crate::discord::{self, Presence};
use crate::i18n::{Lang, Strings, fill};
use crate::mc::{java, version};
use crate::queue::{Manager, State};
use crate::sys;

// ------------------------------------------------------------------ palette

const BG: Color32 = Color32::from_rgb(23, 24, 27);
const SURFACE: Color32 = Color32::from_rgb(30, 32, 36);
const SURFACE_HI: Color32 = Color32::from_rgb(38, 42, 47);
const BORDER: Color32 = Color32::from_rgb(46, 51, 57);
const TEXT: Color32 = Color32::from_rgb(230, 232, 235);
const MUTED: Color32 = Color32::from_rgb(140, 147, 156);
const AMBER: Color32 = Color32::from_rgb(242, 179, 61);
const AMBER_DEEP: Color32 = Color32::from_rgb(150, 104, 22);
const GREEN: Color32 = Color32::from_rgb(111, 191, 115);
const RED: Color32 = Color32::from_rgb(224, 108, 108);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Accounts,
    Instances,
    Settings,
}

// ----------------------------------------------------------------- dialogues

enum Dialog {
    None,
    Account(AccountForm),
    Bulk(BulkForm),
    Microsoft(MsForm),
}

struct AccountForm {
    original: Option<String>,
    name: String,
    version: String,
    xmx: String,
    instance: String,
    premium: bool,
    error: String,
}

impl AccountForm {
    fn new() -> Self {
        Self {
            original: None,
            name: String::new(),
            version: String::new(),
            xmx: String::new(),
            instance: String::new(),
            premium: false,
            error: String::new(),
        }
    }

    fn edit(account: &Account) -> Self {
        Self {
            original: Some(account.name.clone()),
            name: account.name.clone(),
            version: account.version.clone().unwrap_or_default(),
            xmx: account.xmx_mb.map(|v| v.to_string()).unwrap_or_default(),
            instance: account.instance.clone(),
            premium: account.is_premium(),
            error: String::new(),
        }
    }
}

struct BulkForm {
    text: String,
    prefix: String,
    count: usize,
}

/// État partagé avec le thread de connexion Microsoft.
struct MsForm {
    client_id: String,
    status: Arc<Mutex<String>>,
    flow: Arc<Mutex<Option<DeviceFlow>>>,
    outcome: Arc<Mutex<Option<Result<Account, String>>>>,
    stop: Arc<AtomicBool>,
    running: bool,
}

impl MsForm {
    fn new(client_id: String, s: &'static Strings) -> Self {
        Self {
            client_id,
            status: Arc::new(Mutex::new(s.ms_intro.to_string())),
            flow: Arc::new(Mutex::new(None)),
            outcome: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            running: false,
        }
    }
}

// ---------------------------------------------------------------- application

pub struct App {
    settings: Settings,
    accounts: Vec<Account>,
    manager: Manager,
    versions: Vec<String>,
    java_hint: String,
    java_ok: bool,
    tab: Tab,
    dialog: Dialog,
    advice: String,
    show_log: bool,
    presence: Presence,
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let settings = Settings::load();
        let repaint = ctx.clone();
        let manager = Manager::new(move || repaint.request_repaint());
        manager.set_lang(settings.lang);

        let mut app = Self {
            versions: version::list_versions(&settings.mc_dir),
            accounts: load_accounts(),
            settings,
            manager,
            java_hint: String::new(),
            java_ok: true,
            tab: Tab::Accounts,
            dialog: Dialog::None,
            advice: String::new(),
            show_log: true,
            presence: Presence::new(),
        };
        app.presence
            .configure(&app.settings.discord_app_id, app.settings.discord_enabled);
        if app.settings.version.is_empty() || !app.versions.contains(&app.settings.version) {
            app.settings.version = app.versions.first().cloned().unwrap_or_default();
        }
        app.refresh_java_hint();
        app.greet();
        app
    }

    fn s(&self) -> &'static Strings {
        self.settings.s()
    }

    fn greet(&self) {
        let (total, avail) = sys::memory_mb();
        let (fit, _) = self.settings.room_for_more();
        let s = self.s();
        self.manager.log(fill(
            s.log_versions_found,
            &[
                &self.versions.len().to_string(),
                &self.settings.mc_dir.join("versions").display().to_string(),
            ],
        ));
        self.manager.log(fill(
            s.log_ram_summary,
            &[
                &avail.to_string(),
                &total.to_string(),
                &fit.to_string(),
                &self.settings.xmx_mb.to_string(),
            ],
        ));
    }

    fn refresh_versions(&mut self) {
        self.versions = version::list_versions(&self.settings.mc_dir);
        if !self.versions.contains(&self.settings.version) {
            self.settings.version = self.versions.first().cloned().unwrap_or_default();
        }
        self.refresh_java_hint();
    }

    fn refresh_java_hint(&mut self) {
        let s = self.s();
        self.java_ok = true;
        if self.settings.version.is_empty() {
            self.java_hint = s.no_version.to_string();
            self.java_ok = false;
            return;
        }
        self.java_hint = match version::resolve(&self.settings.mc_dir, &self.settings.version) {
            Err(message) => {
                self.java_ok = false;
                message
            }
            Ok(resolved) => {
                let path = java::find_java(&resolved, false);
                let runtime = path
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "?".into());
                let mut hint = fill(s.java_line, &[&resolved.java_major.to_string(), &runtime]);
                if !path.is_file() {
                    self.java_ok = false;
                    hint = format!("{hint} · {}", s.java_missing);
                }
                if resolved.chain.len() > 1 {
                    hint = format!(
                        "{hint} · {}",
                        fill(s.inherits, &[&resolved.chain[1..].join(" < ")])
                    );
                }
                hint
            }
        };
    }

    fn save_all(&self) {
        let _ = self.settings.save();
        let _ = save_accounts(&self.accounts);
    }

    fn checked_count(&self) -> usize {
        self.accounts.iter().filter(|a| a.selected).count()
    }

    /// Ajoute le compte, ou remplace celui qui porte déjà ce pseudo.
    fn upsert(&mut self, mut account: Account) {
        if let Some(index) = self.accounts.iter().position(|a| a.name == account.name) {
            let old = &self.accounts[index];
            if !old.instance.is_empty() {
                account.instance = old.instance.clone();
            }
            account.version = old.version.clone();
            account.xmx_mb = old.xmx_mb;
            self.accounts[index] = account;
        } else {
            self.accounts.push(account);
        }
        let _ = save_accounts(&self.accounts);
    }

    fn launch_selected(&mut self) {
        let s = self.s();
        let chosen: Vec<Account> = self
            .accounts
            .iter()
            .filter(|a| a.selected)
            .cloned()
            .collect();
        if chosen.is_empty() {
            self.manager.log(s.log_no_account);
            return;
        }
        if self.settings.version.is_empty() {
            self.manager.log(s.log_no_version);
            return;
        }
        let (fit, avail) = self.settings.room_for_more();
        if !self.settings.ignore_ram_guard && fit < chosen.len() {
            self.manager.log(fill(
                s.log_room_warning,
                &[&avail.to_string(), &fit.to_string()],
            ));
        }
        self.save_all();
        for account in chosen {
            let version = account
                .version
                .clone()
                .unwrap_or_else(|| self.settings.version.clone());
            self.manager
                .enqueue(account, version, self.settings.clone());
        }
        self.tab = Tab::Instances;
    }

    /// Propose un nombre d'instances et de cœurs qui tient dans la machine.
    fn autotune(&mut self) {
        let (total, avail) = sys::memory_mb();
        let per = self.settings.xmx_mb + self.settings.overhead_mb;
        let fit = (avail.saturating_sub(self.settings.reserve_mb) / per.max(1)).max(1) as usize;
        // Au-delà de quatre clients, c'est la VRAM qui lâche avant la RAM.
        let capped = fit.min(4);
        self.settings.max_instances = capped;
        self.settings.cores_per_instance = (sys::cpu_count() / capped.max(1)).max(2);
        self.advice = fill(
            match self.settings.lang {
                Lang::Fr => {
                    "RAM libre {0} Mo sur {1}. À {2} Mo par instance : {3} tiennent en \
                             mémoire, plafonné à {4} pour la carte graphique, {5} cœurs chacune."
                }
                Lang::En => {
                    "{0} MB free out of {1}. At {2} MB each: {3} fit in memory, capped at \
                             {4} for the GPU, {5} cores apiece."
                }
            },
            &[
                &avail.to_string(),
                &total.to_string(),
                &self.settings.xmx_mb.to_string(),
                &fit.to_string(),
                &capped.to_string(),
                &self.settings.cores_per_instance.to_string(),
            ],
        );
    }

    /// Reprend pseudos et UUID des comptes du launcher officiel.
    fn import_official(&mut self) {
        let s = self.s();
        let path = self.settings.mc_dir.join("launcher_accounts.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.manager
                .log(fill(s.log_import_failed, &[&path.display().to_string()]));
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            self.manager
                .log(fill(s.log_import_failed, &[&path.display().to_string()]));
            return;
        };
        let mut added = 0;
        if let Some(accounts) = value.get("accounts").and_then(|a| a.as_object()) {
            for entry in accounts.values() {
                let profile = entry.get("minecraftProfile");
                let name = profile
                    .and_then(|p| p.get("name"))
                    .and_then(|n| n.as_str())
                    .or_else(|| entry.get("username").and_then(|n| n.as_str()));
                let Some(name) = name else { continue };
                if self.accounts.iter().any(|a| a.name == name) {
                    continue;
                }
                let uuid = profile
                    .and_then(|p| p.get("id"))
                    .and_then(|i| i.as_str())
                    .map(crate::auth::dashed)
                    .unwrap_or_else(|| crate::auth::offline_uuid(name));
                self.accounts.push(Account {
                    uuid,
                    instance: sanitize(name),
                    ..Account::offline(name)
                });
                added += 1;
            }
        }
        let _ = save_accounts(&self.accounts);
        self.manager
            .log(fill(s.log_imported, &[&added.to_string()]));
    }

    /// Ce que Discord montre : des nombres et une version, pas de pseudos.
    fn activity(&self) -> Option<discord::Activity> {
        if !self.settings.discord_enabled || self.settings.discord_app_id.is_empty() {
            return None;
        }
        let s = self.s();
        let (running, pending, start, version) = self
            .manager
            .shared
            .instances
            .lock()
            .map(|list| {
                let live: Vec<_> = list
                    .iter()
                    .filter(|i| matches!(i.state, State::Running | State::Starting))
                    .collect();
                let start = live.iter().filter_map(|i| i.started_epoch).min();
                let version = live.first().map(|i| i.version.clone());
                (
                    live.len(),
                    list.iter().filter(|i| i.state.is_pending()).count(),
                    start,
                    version,
                )
            })
            .unwrap_or((0, 0, None, None));

        let version = version.unwrap_or_else(|| self.settings.version.clone());
        let on_version = if version.is_empty() {
            String::new()
        } else {
            fill(s.rp_on_version, &[&version])
        };
        Some(if running > 0 {
            discord::Activity {
                details: fill(s.rp_running, &[&running.to_string()]),
                state: on_version,
                start,
            }
        } else if pending > 0 {
            discord::Activity {
                details: fill(s.rp_queued, &[&pending.to_string()]),
                state: on_version,
                start: None,
            }
        } else {
            discord::Activity {
                details: s.rp_idle.to_string(),
                state: on_version,
                start: None,
            }
        })
    }

    fn open_path(path: &std::path::Path) {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("explorer")
                .arg(path)
                .creation_flags(0x0800_0000)
                .spawn();
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("xdg-open").arg(path).spawn();
        }
    }
}

// ------------------------------------------------------------------- eframe

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        for account in self.manager.take_refreshed() {
            self.upsert(account);
        }
        let ctx = ui.ctx().clone();

        self.nav_bar(ui);
        self.launch_bar(ui);
        self.status_bar(ui);
        if self.show_log {
            self.journal(ui);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::same(14)),
            )
            .show(ui, |ui| match self.tab {
                Tab::Accounts => self.accounts_tab(ui),
                Tab::Instances => self.instances_tab(ui),
                Tab::Settings => self.settings_tab(ui),
            });

        self.dialogs(&ctx);
        self.presence.set(self.activity());
        if self.manager.shared.running_count() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self) {
        self.save_all();
        self.presence.shutdown();
        self.manager.shutdown();
    }
}

// ------------------------------------------------------------------ panneaux

impl App {
    fn nav_bar(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        let current = self.tab;
        let mut next = current;
        egui::Panel::top("nav")
            .frame(
                egui::Frame::default()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        hexagon(ui, 22.0, AMBER);
                        ui.add_space(8.0);
                        ui.label(RichText::new("RUCHE").size(19.0).strong().color(TEXT));
                        ui.add_space(6.0);
                        ui.label(RichText::new(s.tagline).color(MUTED).size(12.0));
                    },
                    |ui| {
                        // de droite a gauche : on pose les onglets a l'envers
                        for (tab, label) in [
                            (Tab::Settings, s.tab_settings),
                            (Tab::Instances, s.tab_instances),
                            (Tab::Accounts, s.tab_accounts),
                        ] {
                            let active = current == tab;
                            let text = RichText::new(label).size(14.0).color(if active {
                                TEXT
                            } else {
                                MUTED
                            });
                            let button = egui::Button::new(text)
                                .fill(if active {
                                    SURFACE_HI
                                } else {
                                    Color32::TRANSPARENT
                                })
                                .stroke(if active {
                                    Stroke::new(1.0, AMBER_DEEP)
                                } else {
                                    Stroke::NONE
                                })
                                .corner_radius(CornerRadius::same(8))
                                .min_size(Vec2::new(0.0, 30.0));
                            if ui.add(button).clicked() {
                                next = tab;
                            }
                            ui.add_space(4.0);
                        }
                    },
                );
            });
        self.tab = next;
    }

    fn launch_bar(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        let versions = self.versions.clone();
        let count = self.checked_count();
        let java_hint = self.java_hint.clone();
        let java_ok = self.java_ok;
        let has_version = !self.settings.version.is_empty();
        let settings = &mut self.settings;
        let mut refresh = false;
        let mut version_changed = false;
        let mut launch = false;
        egui::Panel::top("hero")
            .frame(
                egui::Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ui, |ui| {
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(s.version).color(MUTED).size(12.0));
                                egui::ComboBox::from_id_salt("version")
                                    .selected_text(
                                        RichText::new(if settings.version.is_empty() {
                                            "—".to_string()
                                        } else {
                                            settings.version.clone()
                                        })
                                        .size(15.0)
                                        .strong(),
                                    )
                                    .width(230.0)
                                    .show_ui(ui, |ui| {
                                        for name in &versions {
                                            if ui
                                                .selectable_value(
                                                    &mut settings.version,
                                                    name.clone(),
                                                    name,
                                                )
                                                .clicked()
                                            {
                                                version_changed = true;
                                            }
                                        }
                                    });
                                if ui.small_button(s.refresh).clicked() {
                                    refresh = true;
                                }
                                ui.add_space(14.0);
                                ui.label(RichText::new(s.server).color(MUTED).size(12.0));
                                ui.add(
                                    egui::TextEdit::singleline(&mut settings.server)
                                        .desired_width(190.0)
                                        .hint_text(s.server_hint),
                                );
                            });
                            ui.add_space(2.0);
                            ui.label(RichText::new(&java_hint).size(11.5).color(if java_ok {
                                MUTED
                            } else {
                                RED
                            }));
                        });
                    },
                    |ui| {
                        let enabled = count > 0 && has_version;
                        ui.vertical(|ui| {
                            ui.set_width(170.0);
                            let button = egui::Button::new(
                                RichText::new(format!("▶  {}", s.launch))
                                    .size(16.0)
                                    .strong()
                                    .color(if enabled {
                                        Color32::from_rgb(28, 24, 12)
                                    } else {
                                        MUTED
                                    }),
                            )
                            .fill(if enabled { AMBER } else { SURFACE })
                            .corner_radius(CornerRadius::same(10))
                            .min_size(Vec2::new(170.0, 42.0));
                            if ui.add_enabled(enabled, button).clicked() {
                                launch = true;
                            }
                            let text = if count == 0 {
                                s.launch_none.to_string()
                            } else {
                                fill(s.launch_count, &[&count.to_string()])
                            };
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new(text).size(11.0).color(MUTED));
                            });
                        });
                    },
                );
            });
        if refresh {
            self.refresh_versions();
        } else if version_changed {
            self.refresh_java_hint();
        }
        if launch {
            self.launch_selected();
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        let running = self.manager.shared.running_count();
        let pending = self.manager.shared.pending_count();
        let reserve = self.settings.reserve_mb;
        let mut show_log = self.show_log;
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::default()
                    .fill(SURFACE)
                    .inner_margin(Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                egui::Sides::new().show(
                    ui,
                    |ui| {
                        let (total, avail) = sys::memory_mb();
                        let used = total.saturating_sub(avail);
                        let ratio = if total > 0 {
                            used as f32 / total as f32
                        } else {
                            0.0
                        };
                        ui.add(
                            egui::ProgressBar::new(ratio)
                                .desired_width(230.0)
                                .corner_radius(CornerRadius::same(5))
                                .fill(if ratio > 0.9 { RED } else { AMBER })
                                .text(
                                    RichText::new(fill(
                                        s.status_ram,
                                        &[&used.to_string(), &total.to_string()],
                                    ))
                                    .size(11.0),
                                ),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            RichText::new(fill(
                                s.status_counts,
                                &[
                                    &running.to_string(),
                                    &pending.to_string(),
                                    &reserve.to_string(),
                                ],
                            ))
                            .size(12.0)
                            .color(MUTED),
                        );
                    },
                    |ui| {
                        if ui
                            .selectable_label(show_log, RichText::new(s.show_log).size(12.0))
                            .clicked()
                        {
                            show_log = !show_log;
                        }
                    },
                );
            });
        self.show_log = show_log;
    }

    fn journal(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("journal")
            .resizable(true)
            .default_size(140.0)
            .frame(
                egui::Frame::default()
                    .fill(Color32::from_rgb(19, 20, 23))
                    .inner_margin(Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let lines: Vec<String> = self
                            .manager
                            .shared
                            .log
                            .lock()
                            .map(|log| log.iter().cloned().collect())
                            .unwrap_or_default();
                        for line in lines {
                            ui.label(RichText::new(line).monospace().size(11.0).color(MUTED));
                        }
                    });
            });
    }
}

// -------------------------------------------------------------------- onglets

impl App {
    fn accounts_tab(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        ui.horizontal_wrapped(|ui| {
            if ui.button(s.add_account).clicked() {
                self.dialog = Dialog::Account(AccountForm::new());
            }
            if ui.button(s.add_bulk).clicked() {
                self.dialog = Dialog::Bulk(BulkForm {
                    text: String::new(),
                    prefix: "Alt".into(),
                    count: 4,
                });
            }
            if ui
                .add(
                    egui::Button::new(RichText::new(s.add_microsoft).color(TEXT))
                        .fill(SURFACE_HI)
                        .stroke(Stroke::new(1.0, AMBER_DEEP)),
                )
                .clicked()
            {
                self.dialog =
                    Dialog::Microsoft(MsForm::new(self.settings.azure_client_id.clone(), s));
            }
            if ui.button(s.import_launcher).clicked() {
                self.import_official();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(s.deselect_all).clicked() {
                    self.accounts.iter_mut().for_each(|a| a.selected = false);
                }
                if ui.button(s.select_all).clicked() {
                    self.accounts.iter_mut().for_each(|a| a.selected = true);
                }
            });
        });
        ui.add_space(10.0);

        if self.accounts.is_empty() {
            empty_state(ui, s.no_accounts_title, s.no_accounts_hint);
            return;
        }

        let mut edit: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut open: Option<usize> = None;
        let default_version = self.settings.version.clone();

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for (index, account) in self.accounts.iter_mut().enumerate() {
                    card(ui, |ui| {
                        egui::Sides::new().show(
                            ui,
                            |ui| {
                                ui.checkbox(&mut account.selected, "");
                                ui.add_space(2.0);
                                avatar(ui, &account.name, &account.uuid);
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(&account.name).size(15.0).strong());
                                        if account.is_premium() {
                                            chip(ui, s.kind_premium, AMBER);
                                        } else {
                                            chip(ui, s.kind_offline, MUTED);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        let version = account
                                            .version
                                            .clone()
                                            .unwrap_or_else(|| default_version.clone());
                                        ui.label(RichText::new(version).size(11.5).color(MUTED));
                                        ui.label(RichText::new("·").color(MUTED));
                                        let ram = match account.xmx_mb {
                                            Some(mb) => format!("{mb} Mo"),
                                            None => s.chip_ram_global.to_string(),
                                        };
                                        ui.label(RichText::new(ram).size(11.5).color(MUTED));
                                        if account.is_premium() {
                                            ui.label(RichText::new("·").color(MUTED));
                                            let (text, color) =
                                                session_text(account.session_left(), s);
                                            ui.label(RichText::new(text).size(11.5).color(color));
                                        }
                                    });
                                });
                            },
                            |ui| {
                                if ui.button(s.delete).clicked() {
                                    remove = Some(index);
                                }
                                if ui.button(s.open_folder).clicked() {
                                    open = Some(index);
                                }
                                if ui.button(s.edit).clicked() {
                                    edit = Some(index);
                                }
                            },
                        );
                    });
                    ui.add_space(8.0);
                }
            });

        if let Some(index) = edit {
            self.dialog = Dialog::Account(AccountForm::edit(&self.accounts[index]));
        }
        if let Some(index) = open {
            let dir = self.accounts[index].game_dir(&self.settings);
            let _ = std::fs::create_dir_all(&dir);
            Self::open_path(&dir);
        }
        if let Some(index) = remove {
            self.accounts.remove(index);
            let _ = save_accounts(&self.accounts);
        }
    }

    fn instances_tab(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        ui.horizontal_wrapped(|ui| {
            if ui.button(s.clear_queue).clicked() {
                self.manager.clear_queue();
            }
            if ui
                .add(
                    egui::Button::new(RichText::new(s.stop_all).color(TEXT))
                        .fill(Color32::from_rgb(96, 44, 44)),
                )
                .clicked()
            {
                self.manager.clear_queue();
                self.manager.kill_all();
            }
            if ui.button(s.cleanup).clicked() {
                self.manager.forget_finished();
            }
        });
        ui.add_space(10.0);

        struct Row {
            id: u64,
            account: String,
            version: String,
            state: State,
            rss: u64,
            uptime: String,
            pid: String,
            has_log: bool,
        }
        let rows: Vec<Row> = self
            .manager
            .shared
            .instances
            .lock()
            .map(|list| {
                list.iter()
                    .map(|i| Row {
                        id: i.id,
                        account: i.account.clone(),
                        version: i.version.clone(),
                        state: i.state,
                        rss: i.rss_mb,
                        uptime: i.uptime(),
                        pid: i.pid.map(|p| p.to_string()).unwrap_or_default(),
                        has_log: i.log_path.is_some(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        if rows.is_empty() {
            empty_state(ui, s.no_instances_title, s.no_instances_hint);
            return;
        }

        let mut stop: Option<u64> = None;
        let mut show_log: Option<u64> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                for row in &rows {
                    let color = match row.state {
                        State::Running => GREEN,
                        State::Crashed => RED,
                        State::Queued | State::WaitingRoom | State::Starting => AMBER,
                        _ => MUTED,
                    };
                    card(ui, |ui| {
                        egui::Sides::new().show(
                            ui,
                            |ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                                ui.painter().circle_filled(rect.center(), 5.0, color);
                                ui.add_space(8.0);
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(&row.account).size(15.0).strong());
                                        ui.label(
                                            RichText::new(row.state.label(s))
                                                .size(12.0)
                                                .color(color),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(&row.version).size(11.5).color(MUTED),
                                        );
                                        if row.rss > 0 {
                                            ui.label(RichText::new("·").color(MUTED));
                                            ui.label(
                                                RichText::new(format!("{} {}", row.rss, s.col_ram))
                                                    .size(11.5)
                                                    .color(MUTED),
                                            );
                                        }
                                        if !row.uptime.is_empty() {
                                            ui.label(RichText::new("·").color(MUTED));
                                            ui.label(
                                                RichText::new(&row.uptime).size(11.5).color(MUTED),
                                            );
                                        }
                                        if !row.pid.is_empty() {
                                            ui.label(RichText::new("·").color(MUTED));
                                            ui.label(
                                                RichText::new(format!("pid {}", row.pid))
                                                    .size(11.5)
                                                    .color(MUTED),
                                            );
                                        }
                                    });
                                });
                            },
                            |ui| {
                                if ui
                                    .add_enabled(row.has_log, egui::Button::new(s.view_log))
                                    .clicked()
                                {
                                    show_log = Some(row.id);
                                }
                                if ui
                                    .add_enabled(!row.state.is_over(), egui::Button::new(s.stop))
                                    .clicked()
                                {
                                    stop = Some(row.id);
                                }
                            },
                        );
                    });
                    ui.add_space(8.0);
                }
            });

        if let Some(id) = stop {
            self.manager.kill(id);
        }
        if let Some(id) = show_log {
            let path = self.manager.shared.instances.lock().ok().and_then(|list| {
                list.iter()
                    .find(|i| i.id == id)
                    .and_then(|i| i.log_path.clone())
            });
            match path {
                Some(path) if path.is_file() => Self::open_path(&path),
                _ => self.manager.log(s.log_no_log),
            }
        }
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui) {
        let s = self.s();
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                // ------------------------------------------------- mémoire
                section(ui, s.sec_memory, |ui| {
                    field(ui, s.set_xmx, s.set_xmx_hint, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.xmx_mb)
                                .range(512..=32768)
                                .speed(64)
                                .suffix(s.unit_mb),
                        );
                    });
                    field(ui, s.set_reserve, s.set_reserve_hint, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.reserve_mb)
                                .range(0..=65536)
                                .speed(128)
                                .suffix(s.unit_mb),
                        );
                    });
                    field(ui, s.set_max, "", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.max_instances).range(1..=16),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(s.autotune).clicked() {
                            self.autotune();
                        }
                        if !self.advice.is_empty() {
                            ui.label(RichText::new(&self.advice).size(11.5).color(AMBER));
                        }
                    });
                });

                // -------------------------------------------------- cadence
                section(ui, s.sec_pace, |ui| {
                    field(ui, s.set_stagger_min, s.set_stagger_hint, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.stagger_min_s)
                                .range(0..=120)
                                .suffix(s.unit_s),
                        );
                    });
                    field(ui, s.set_stagger_max, "", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.stagger_max_s)
                                .range(15..=900)
                                .suffix(s.unit_s),
                        );
                    });
                    field(ui, s.set_timeout, "", |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.wait_timeout_s)
                                .range(30..=3600)
                                .speed(10)
                                .suffix(s.unit_s),
                        );
                    });
                });

                // -------------------------------------------------- système
                section(ui, s.sec_system, |ui| {
                    field(ui, s.set_priority, "", |ui| {
                        egui::ComboBox::from_id_salt("priorite")
                            .selected_text(priority_label(self.settings.priority, s))
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for p in Priority::ALL {
                                    ui.selectable_value(
                                        &mut self.settings.priority,
                                        p,
                                        priority_label(p, s),
                                    );
                                }
                            });
                    });
                    field(ui, s.set_cores, s.set_cores_hint, |ui| {
                        ui.add(
                            egui::DragValue::new(&mut self.settings.cores_per_instance)
                                .range(0..=64),
                        );
                    });
                    field(ui, s.set_ignore_guard, s.set_ignore_guard_hint, |ui| {
                        ui.checkbox(&mut self.settings.ignore_ram_guard, "");
                    });
                });

                // ------------------------------------------------------ jeu
                section(ui, s.sec_game, |ui| {
                    field(ui, s.set_window, "", |ui| {
                        ui.add(egui::DragValue::new(&mut self.settings.width).range(320..=7680));
                        ui.label("×");
                        ui.add(egui::DragValue::new(&mut self.settings.height).range(240..=4320));
                    });
                    field(ui, s.set_fullscreen, "", |ui| {
                        ui.checkbox(&mut self.settings.fullscreen, "");
                    });
                    field(ui, s.set_low, "", |ui| {
                        ui.checkbox(&mut self.settings.low_settings, "");
                    });
                    field(ui, s.set_share, "", |ui| {
                        ui.checkbox(&mut self.settings.share_mods, "");
                    });
                    field(ui, s.set_add_server, "", |ui| {
                        ui.checkbox(&mut self.settings.add_server_entry, "");
                    });
                    field(ui, s.set_extra_jvm, "", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.extra_jvm)
                                .desired_width(260.0),
                        );
                    });
                });

                // -------------------------------------------------- premium
                section(ui, s.sec_premium, |ui| {
                    field(ui, s.set_azure, "", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.azure_client_id)
                                .desired_width(300.0),
                        );
                    });
                    ui.label(RichText::new(s.azure_help).size(11.0).color(MUTED));
                });

                // ---------------------------------------------------- discord
                section(ui, s.sec_discord, |ui| {
                    let before = (
                        self.settings.discord_enabled,
                        self.settings.discord_app_id.clone(),
                    );
                    field(ui, s.set_discord, s.set_discord_hint, |ui| {
                        ui.checkbox(&mut self.settings.discord_enabled, "");
                    });
                    field(ui, s.set_discord_app, "", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.settings.discord_app_id)
                                .desired_width(260.0),
                        );
                    });
                    if before
                        != (
                            self.settings.discord_enabled,
                            self.settings.discord_app_id.clone(),
                        )
                    {
                        self.presence.configure(
                            &self.settings.discord_app_id,
                            self.settings.discord_enabled,
                        );
                        let _ = self.settings.save();
                    }
                    let (text, color) = match self.presence.status() {
                        discord::Status::Off => (s.discord_off.to_string(), MUTED),
                        discord::Status::Connecting => (s.discord_connecting.to_string(), MUTED),
                        discord::Status::Connected => (s.discord_connected.to_string(), GREEN),
                        discord::Status::Unavailable(why) => {
                            (fill(s.discord_unavailable, &[&why]), AMBER)
                        }
                    };
                    ui.label(RichText::new(text).size(11.5).color(color));
                    ui.label(RichText::new(s.discord_help).size(11.0).color(MUTED));
                });

                // ------------------------------------------- langue et dossiers
                section(ui, s.sec_appearance, |ui| {
                    let previous = self.settings.lang;
                    field(ui, s.set_language, "", |ui| {
                        egui::ComboBox::from_id_salt("langue")
                            .selected_text(self.settings.lang.label())
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for lang in Lang::ALL {
                                    ui.selectable_value(
                                        &mut self.settings.lang,
                                        lang,
                                        lang.label(),
                                    );
                                }
                            });
                    });
                    if self.settings.lang != previous {
                        self.manager.set_lang(self.settings.lang);
                        self.refresh_java_hint();
                        self.advice.clear();
                        let _ = self.settings.save();
                    }
                    field(ui, s.set_mc_dir, "", |ui| {
                        let path = self.settings.mc_dir.display().to_string();
                        ui.label(RichText::new(path).size(11.5).color(MUTED));
                        if ui.small_button(s.open).clicked() {
                            Self::open_path(&self.settings.mc_dir.clone());
                        }
                    });
                    field(ui, s.set_instances_dir, "", |ui| {
                        let path = self.settings.instances_dir.display().to_string();
                        ui.label(RichText::new(path).size(11.5).color(MUTED));
                        if ui.small_button(s.open).clicked() {
                            let dir = self.settings.instances_dir.clone();
                            let _ = std::fs::create_dir_all(&dir);
                            Self::open_path(&dir);
                        }
                    });
                });

                ui.add_space(4.0);
                if ui
                    .add(
                        egui::Button::new(RichText::new(s.save).strong())
                            .fill(SURFACE_HI)
                            .min_size(Vec2::new(120.0, 30.0)),
                    )
                    .clicked()
                {
                    self.save_all();
                    self.manager.log(s.log_settings_saved);
                }
                ui.add_space(8.0);
            });
    }
}

// ----------------------------------------------------------------- dialogues

impl App {
    fn dialogs(&mut self, ctx: &egui::Context) {
        let close = match &mut self.dialog {
            Dialog::None => false,
            Dialog::Account(_) => self.account_dialog(ctx),
            Dialog::Bulk(_) => self.bulk_dialog(ctx),
            Dialog::Microsoft(_) => self.microsoft_dialog(ctx),
        };
        if close {
            self.dialog = Dialog::None;
        }
    }

    fn account_dialog(&mut self, ctx: &egui::Context) -> bool {
        let s = self.s();
        let versions = self.versions.clone();
        let Dialog::Account(form) = &mut self.dialog else {
            return false;
        };
        let mut close = false;
        let mut commit = false;
        let mut open = true;
        let title = if form.original.is_some() {
            s.dlg_edit_account
        } else {
            s.dlg_new_account
        };

        modal(ctx, title, &mut open, |ui| {
            egui::Grid::new("compte")
                .num_columns(2)
                .spacing([10.0, 10.0])
                .show(ui, |ui| {
                    ui.label(s.field_name);
                    ui.add_enabled(
                        !form.premium,
                        egui::TextEdit::singleline(&mut form.name).desired_width(240.0),
                    );
                    ui.end_row();

                    ui.label(s.field_version);
                    egui::ComboBox::from_id_salt("version-compte")
                        .selected_text(if form.version.is_empty() {
                            s.field_version_default.to_string()
                        } else {
                            form.version.clone()
                        })
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut form.version,
                                String::new(),
                                s.field_version_default,
                            );
                            for name in &versions {
                                ui.selectable_value(&mut form.version, name.clone(), name);
                            }
                        });
                    ui.end_row();

                    ui.label(s.field_ram);
                    ui.add(
                        egui::TextEdit::singleline(&mut form.xmx)
                            .desired_width(240.0)
                            .hint_text(s.field_ram_hint),
                    );
                    ui.end_row();

                    ui.label(s.field_instance);
                    ui.add(
                        egui::TextEdit::singleline(&mut form.instance)
                            .desired_width(240.0)
                            .hint_text(s.field_instance_hint),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            let note = if form.premium {
                s.note_premium
            } else {
                s.note_offline
            };
            ui.label(RichText::new(note).size(11.5).color(MUTED));
            if !form.error.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(&form.error).color(RED));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(s.validate)
                                .strong()
                                .color(Color32::from_rgb(28, 24, 12)),
                        )
                        .fill(AMBER),
                    )
                    .clicked()
                {
                    commit = true;
                }
                if ui.button(s.cancel).clicked() {
                    close = true;
                }
            });
        });

        if commit {
            let name = form.name.trim().to_string();
            if name.is_empty() {
                form.error = s.err_name_required.to_string();
                return false;
            }
            let duplicate = self
                .accounts
                .iter()
                .any(|a| a.name == name && Some(&a.name) != form.original.as_ref());
            if duplicate {
                form.error = s.err_name_exists.to_string();
                return false;
            }
            let version = (!form.version.is_empty()).then(|| form.version.clone());
            let xmx = form.xmx.trim().parse::<u64>().ok();
            let instance = if form.instance.trim().is_empty() {
                sanitize(&name)
            } else {
                form.instance.trim().to_string()
            };
            let premium = form.premium;
            match form.original.clone() {
                Some(original) => {
                    if let Some(account) = self.accounts.iter_mut().find(|a| a.name == original) {
                        if !premium {
                            account.name = name;
                            account.uuid = crate::auth::offline_uuid(&account.name);
                        }
                        account.version = version;
                        account.xmx_mb = xmx;
                        account.instance = instance;
                    }
                }
                None => {
                    let mut account = Account::offline(&name);
                    account.version = version;
                    account.xmx_mb = xmx;
                    account.instance = instance;
                    self.accounts.push(account);
                }
            }
            let _ = save_accounts(&self.accounts);
            return true;
        }
        close || !open
    }

    fn bulk_dialog(&mut self, ctx: &egui::Context) -> bool {
        let s = self.s();
        let Dialog::Bulk(form) = &mut self.dialog else {
            return false;
        };
        let mut close = false;
        let mut commit = false;
        let mut open = true;

        modal(ctx, s.dlg_bulk, &mut open, |ui| {
            ui.label(RichText::new(s.bulk_lines).color(MUTED).size(12.0));
            ui.add(
                egui::TextEdit::multiline(&mut form.text)
                    .desired_width(300.0)
                    .desired_rows(9),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(s.bulk_generate).color(MUTED).size(12.0));
                ui.add(egui::TextEdit::singleline(&mut form.prefix).desired_width(90.0));
                ui.add(egui::DragValue::new(&mut form.count).range(1..=64));
                if ui.button(s.bulk_fill).clicked() {
                    form.text = (1..=form.count)
                        .map(|i| format!("{}{i}", form.prefix.trim()))
                        .collect::<Vec<_>>()
                        .join("\n");
                }
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(s.add)
                                .strong()
                                .color(Color32::from_rgb(28, 24, 12)),
                        )
                        .fill(AMBER),
                    )
                    .clicked()
                {
                    commit = true;
                }
                if ui.button(s.cancel).clicked() {
                    close = true;
                }
            });
        });

        if commit {
            let names: Vec<String> = form
                .text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && l.chars().count() <= 16)
                .collect();
            let mut added = 0;
            for name in names {
                if self.accounts.iter().any(|a| a.name == name) {
                    continue;
                }
                self.accounts.push(Account::offline(&name));
                added += 1;
            }
            let _ = save_accounts(&self.accounts);
            self.manager.log(fill(s.log_added, &[&added.to_string()]));
            return true;
        }
        close || !open
    }

    fn microsoft_dialog(&mut self, ctx: &egui::Context) -> bool {
        let s = self.s();
        let lang = self.settings.lang;
        let Dialog::Microsoft(form) = &mut self.dialog else {
            return false;
        };
        let mut close = false;
        let mut open = true;
        let mut start = false;

        let code = form
            .flow
            .lock()
            .ok()
            .and_then(|f| f.as_ref().map(|f| f.user_code.clone()));
        let uri = form
            .flow
            .lock()
            .ok()
            .and_then(|f| f.as_ref().map(|f| f.verification_uri.clone()));

        modal(ctx, s.dlg_microsoft, &mut open, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(s.set_azure).color(MUTED).size(12.0));
                ui.add(
                    egui::TextEdit::singleline(&mut form.client_id)
                        .desired_width(280.0)
                        .hint_text("00000000-0000-0000-0000-000000000000"),
                );
            });

            ui.add_space(12.0);
            egui::Frame::default()
                .fill(Color32::from_rgb(19, 20, 23))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::symmetric(16, 14))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(code.clone().unwrap_or_else(|| "— — — — —".into()))
                                .monospace()
                                .size(30.0)
                                .strong()
                                .color(AMBER),
                        );
                    });
                });
            ui.add_space(8.0);

            let status = form.status.lock().map(|s| s.clone()).unwrap_or_default();
            ui.label(RichText::new(status).size(12.0).color(MUTED));

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !form.running,
                        egui::Button::new(
                            RichText::new(s.ms_get_code)
                                .strong()
                                .color(Color32::from_rgb(28, 24, 12)),
                        )
                        .fill(AMBER),
                    )
                    .clicked()
                {
                    start = true;
                }
                if ui
                    .add_enabled(code.is_some(), egui::Button::new(s.ms_open_page))
                    .clicked()
                    && let Some(uri) = &uri
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(uri));
                }
                if ui
                    .add_enabled(code.is_some(), egui::Button::new(s.ms_copy))
                    .clicked()
                    && let Some(code) = &code
                {
                    ui.ctx().copy_text(code.clone());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(s.close).clicked() {
                        close = true;
                    }
                });
            });
            ui.add_space(8.0);
            ui.label(RichText::new(s.azure_help).size(11.0).color(MUTED));
        });

        if start {
            let client_id = form.client_id.trim().to_string();
            if client_id.is_empty() {
                if let Ok(mut status) = form.status.lock() {
                    *status = s.ms_need_id.to_string();
                }
            } else {
                self.settings.azure_client_id = client_id.clone();
                let _ = self.settings.save();
                form.running = true;
                let (status, flow, outcome, stop) = (
                    Arc::clone(&form.status),
                    Arc::clone(&form.flow),
                    Arc::clone(&form.outcome),
                    Arc::clone(&form.stop),
                );
                if let Ok(mut slot) = status.lock() {
                    *slot = s.ms_asking.to_string();
                }
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let status_code = Arc::clone(&status);
                    let repaint_code = repaint.clone();
                    let repaint_wait = repaint.clone();
                    let stop_flag = Arc::clone(&stop);
                    let clip = repaint.clone();
                    let result = msa::login_device(
                        &client_id,
                        lang,
                        |device| {
                            if let Ok(mut slot) = flow.lock() {
                                *slot = Some(device.clone());
                            }
                            clip.copy_text(device.user_code.clone());
                            if let Ok(mut slot) = status_code.lock() {
                                *slot = fill(s.ms_code_hint, &[&device.verification_uri]);
                            }
                            repaint_code.request_repaint();
                        },
                        move || stop_flag.load(Ordering::Relaxed),
                        |left| {
                            if let Ok(mut slot) = status.lock() {
                                *slot = fill(s.ms_waiting, &[&(left / 60).to_string()]);
                            }
                            repaint_wait.request_repaint();
                        },
                    );
                    if let Ok(mut slot) = outcome.lock() {
                        *slot = Some(result.map_err(|e| e.0));
                    }
                    repaint.request_repaint();
                });
            }
        }

        let outcome = form.outcome.lock().ok().and_then(|mut slot| slot.take());
        match outcome {
            Some(Ok(account)) => {
                self.manager
                    .log(fill(s.log_connected, &[&account.name, &account.uuid]));
                self.upsert(account);
                return true;
            }
            Some(Err(message)) => {
                form.running = false;
                if let Ok(mut status) = form.status.lock() {
                    *status = fill(s.ms_failed, &[&message]);
                }
                self.manager.log(fill(s.log_connect_failed, &[&message]));
            }
            None => {}
        }

        if close || !open {
            form.stop.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }
}

// ------------------------------------------------------------------ briques

/// Carte : le motif de base des listes.
fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::default()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(12, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Bloc de réglages avec son titre.
fn section(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(RichText::new(title).size(13.0).strong().color(AMBER));
    ui.add_space(6.0);
    card(ui, add);
    ui.add_space(12.0);
}

/// Ligne d'un bloc de réglages : intitulé à gauche, contrôle à droite.
fn field(ui: &mut egui::Ui, label: &str, hint: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Sides::new().show(
        ui,
        |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(13.0));
                if !hint.is_empty() {
                    ui.label(RichText::new(hint).size(11.0).color(MUTED));
                }
            });
        },
        |ui| {
            ui.horizontal(|ui| {
                add(ui);
            });
        },
    );
    ui.add_space(8.0);
}

/// Petite étiquette arrondie.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::default()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            28,
        ))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.5).color(color));
        });
}

/// Pastille colorée avec l'initiale : la teinte vient de l'UUID.
fn avatar(ui: &mut egui::Ui, name: &str, uuid: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::hover());
    let seed = uuid
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let color = hue_color(seed % 360);
    ui.painter()
        .circle_filled(rect.center(), 15.0, color.gamma_multiply(0.35));
    let initial = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(14.0),
        color,
    );
}

/// Message centré quand une liste est vide.
fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        hexagon(ui, 40.0, BORDER);
        ui.add_space(10.0);
        ui.label(RichText::new(title).size(16.0).strong().color(MUTED));
        ui.add_space(4.0);
        ui.label(
            RichText::new(hint)
                .size(12.0)
                .color(MUTED.gamma_multiply(0.8)),
        );
    });
}

/// L'alvéole du logo, dessinée à la main : pas d'image à charger.
fn hexagon(ui: &mut egui::Ui, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::hover());
    let center = rect.center();
    let radius = size * 0.5;
    let points = |r: f32| -> Vec<egui::Pos2> {
        (0..6)
            .map(|i| {
                let angle = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::FRAC_PI_3;
                egui::pos2(center.x + r * angle.cos(), center.y + r * angle.sin())
            })
            .collect()
    };
    ui.painter().add(egui::Shape::convex_polygon(
        points(radius),
        Color32::TRANSPARENT,
        Stroke::new(size * 0.16, color),
    ));
    ui.painter().add(egui::Shape::convex_polygon(
        points(radius * 0.34),
        color,
        Stroke::NONE,
    ));
}

/// Fenêtre modale centrée, au même style que les cartes.
fn modal(ctx: &egui::Context, title: &str, open: &mut bool, add: impl FnOnce(&mut egui::Ui)) {
    egui::Window::new(RichText::new(title).size(15.0).strong())
        .collapsible(false)
        .resizable(false)
        .open(open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(
            egui::Frame::default()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(16))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 8],
                    blur: 24,
                    spread: 0,
                    color: Color32::from_black_alpha(120),
                }),
        )
        .show(ctx, |ui| add(ui));
}

/// Duree de session restante : en heures des qu'elle depasse l'heure.
fn session_text(seconds_left: i64, s: &'static Strings) -> (String, Color32) {
    if seconds_left <= 0 {
        return (s.session_expired.to_string(), AMBER);
    }
    let minutes = seconds_left / 60;
    if minutes >= 60 {
        (
            fill(s.session_left_hours, &[&(minutes / 60).to_string()]),
            MUTED,
        )
    } else {
        (fill(s.session_left, &[&minutes.to_string()]), MUTED)
    }
}

fn priority_label(priority: Priority, s: &'static Strings) -> &'static str {
    match priority {
        Priority::Normal => s.prio_normal,
        Priority::BelowNormal => s.prio_below,
        Priority::Idle => s.prio_idle,
    }
}

/// Couleur vive à partir d'une teinte, pour les pastilles de compte.
fn hue_color(hue: u32) -> Color32 {
    let h = hue as f32 / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    Color32::from_rgb(
        (r * 235.0) as u8 + 20,
        (g * 235.0) as u8 + 20,
        (b * 235.0) as u8 + 20,
    )
}

/// Thème sombre, calé sur les couleurs de l'icône.
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::from_rgb(19, 20, 23);
    visuals.faint_bg_color = SURFACE_HI;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = AMBER_DEEP;
    visuals.selection.stroke = Stroke::new(1.0, AMBER);
    visuals.hyperlink_color = AMBER;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.bg_fill = SURFACE_HI;
    visuals.widgets.inactive.weak_bg_fill = SURFACE_HI;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 53, 60);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(48, 53, 60);
    visuals.widgets.active.bg_fill = Color32::from_rgb(58, 64, 72);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(7);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(7);
    visuals.widgets.active.corner_radius = CornerRadius::same(7);
    visuals.window_corner_radius = CornerRadius::same(12);
    ctx.set_visuals(visuals);

    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        style.spacing.button_padding = Vec2::new(10.0, 5.0);
        style.spacing.interact_size.y = 26.0;
        style.visuals.striped = false;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_colours_are_stable_and_bright() {
        for hue in [0, 45, 120, 240, 300, 359] {
            let color = hue_color(hue);
            let brightest = color.r().max(color.g()).max(color.b());
            assert!(brightest > 200, "teinte {hue} trop sombre : {color:?}");
        }
    }

    #[test]
    fn session_duration_switches_to_hours() {
        let s = Lang::Fr.strings();
        assert_eq!(session_text(0, s).0, s.session_expired);
        assert_eq!(session_text(-10, s).0, s.session_expired);
        assert_eq!(session_text(30 * 60, s).0, "session 30 min");
        assert_eq!(session_text(23 * 3600, s).0, "session 23 h");
    }

    #[test]
    fn priority_labels_follow_the_language() {
        assert_ne!(
            priority_label(Priority::Idle, Lang::Fr.strings()),
            priority_label(Priority::Idle, Lang::En.strings())
        );
    }
}
