//! Interface : comptes, reglages, file de lancement et journal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_extras::{Column, Size, StripBuilder, TableBuilder};

use crate::auth::msa::{self, DeviceFlow};
use crate::config::{Account, Priority, Settings, load_accounts, sanitize, save_accounts};
use crate::mc::{java, version};
use crate::queue::{Manager, State};
use crate::sys;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(102, 187, 106);
const WARN: egui::Color32 = egui::Color32::from_rgb(230, 162, 60);
const BAD: egui::Color32 = egui::Color32::from_rgb(229, 115, 115);
const MUTED: egui::Color32 = egui::Color32::from_rgb(150, 156, 165);

/// Fenêtre modale ouverte, s'il y en a une.
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
    session_left: i64,
    error: String,
}

struct BulkForm {
    text: String,
    prefix: String,
    count: usize,
}

/// État partage avec le thread de connexion Microsoft.
struct MsForm {
    client_id: String,
    status: Arc<Mutex<String>>,
    flow: Arc<Mutex<Option<DeviceFlow>>>,
    outcome: Arc<Mutex<Option<Result<Account, String>>>>,
    stop: Arc<AtomicBool>,
    running: bool,
}

impl MsForm {
    fn new(client_id: String) -> Self {
        Self {
            client_id,
            status: Arc::new(Mutex::new(
                "Colle ton identifiant d'application Azure, puis demande un code.".into(),
            )),
            flow: Arc::new(Mutex::new(None)),
            outcome: Arc::new(Mutex::new(None)),
            stop: Arc::new(AtomicBool::new(false)),
            running: false,
        }
    }
}

pub struct App {
    settings: Settings,
    accounts: Vec<Account>,
    manager: Manager,
    versions: Vec<String>,
    java_hint: String,
    selected_account: Option<String>,
    selected_instance: Option<u64>,
    dialog: Dialog,
    advice: String,
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let settings = Settings::load();
        let accounts = load_accounts();
        let repaint_ctx = ctx.clone();
        let manager = Manager::new(move || repaint_ctx.request_repaint());

        let mut app = Self {
            versions: version::list_versions(&settings.mc_dir),
            settings,
            accounts,
            manager,
            java_hint: String::new(),
            selected_account: None,
            selected_instance: None,
            dialog: Dialog::None,
            advice: String::new(),
        };
        if app.settings.version.is_empty() || !app.versions.contains(&app.settings.version) {
            app.settings.version = app.versions.first().cloned().unwrap_or_default();
        }
        app.refresh_java_hint();
        let (total, avail) = sys::memory_mb();
        let (fit, _) = app.settings.room_for_more();
        app.manager.log(format!(
            "{} version(s) trouvée(s) dans {}",
            app.versions.len(),
            app.settings.mc_dir.join("versions").display()
        ));
        app.manager.log(format!(
            "RAM {avail} Mo libres sur {total} — {fit} instance(s) de {} Mo tiennent tout de suite",
            app.settings.xmx_mb
        ));
        app
    }

    fn refresh_versions(&mut self) {
        self.versions = version::list_versions(&self.settings.mc_dir);
        if !self.versions.contains(&self.settings.version) {
            self.settings.version = self.versions.first().cloned().unwrap_or_default();
        }
        self.refresh_java_hint();
    }

    fn refresh_java_hint(&mut self) {
        if self.settings.version.is_empty() {
            self.java_hint = "aucune version installée".into();
            return;
        }
        self.java_hint = match version::resolve(&self.settings.mc_dir, &self.settings.version) {
            Err(message) => message,
            Ok(resolved) => {
                let path = java::find_java(&resolved, false);
                let runtime = path
                    .parent()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "?".into());
                let mut hint = format!("Java {} — {runtime}", resolved.java_major);
                if !path.is_file() {
                    hint.push_str("  (JRE introuvable !)");
                }
                if resolved.chain.len() > 1 {
                    hint.push_str(&format!(
                        "  |  hérite de {}",
                        resolved.chain[1..].join(" < ")
                    ));
                }
                hint
            }
        };
    }

    fn save_all(&self) {
        let _ = self.settings.save();
        let _ = save_accounts(&self.accounts);
    }

    fn account_mut(&mut self, name: &str) -> Option<&mut Account> {
        self.accounts.iter_mut().find(|a| a.name == name)
    }

    /// Ajoute le compte, ou remplace celui qui porte deja ce pseudo.
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
        let chosen: Vec<Account> = self
            .accounts
            .iter()
            .filter(|a| a.selected)
            .cloned()
            .collect();
        if chosen.is_empty() {
            self.manager.log("aucun compte coché");
            return;
        }
        if self.settings.version.is_empty() {
            self.manager.log("aucune version sélectionnée");
            return;
        }
        let (fit, avail) = self.settings.room_for_more();
        if !self.settings.ignore_ram_guard && fit < chosen.len() {
            self.manager.log(format!(
                "{avail} Mo libres : {fit} instance(s) partent tout de suite, les autres \
                 attendent qu'il y ait de la place",
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
    }

    /// Propose un nombre d'instances et de coeurs qui tient dans la machine.
    fn autotune(&mut self) {
        let (total, avail) = sys::memory_mb();
        let per = self.settings.xmx_mb + self.settings.overhead_mb;
        let fit = (avail.saturating_sub(self.settings.reserve_mb) / per.max(1)).max(1) as usize;
        // Au-dela de quatre clients, c'est la VRAM qui lache avant la RAM.
        let capped = fit.min(4);
        self.settings.max_instances = capped;
        self.settings.cores_per_instance = (sys::cpu_count() / capped.max(1)).max(2);
        self.advice = format!(
            "RAM libre {avail} Mo sur {total}. A {} Mo par instance : {fit} tiennent en RAM, \
             plafonné à {capped} pour la carte graphique. {} coeurs par instance.",
            self.settings.xmx_mb, self.settings.cores_per_instance
        );
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Sessions premium renouvelees par la file : on les reporte ici.
        for account in self.manager.take_refreshed() {
            self.upsert(account);
        }

        let ctx = ui.ctx().clone();
        self.top_bar(ui);
        self.settings_panel(ui);
        self.status_bar(ui);
        self.journal_panel(ui);
        self.central(ui);
        self.dialogs(&ctx);

        if self.manager.shared.running_count() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self) {
        self.save_all();
        self.manager.shutdown();
    }
}

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("barre").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.strong("Version");
                let mut changed = false;
                egui::ComboBox::from_id_salt("version")
                    .selected_text(if self.settings.version.is_empty() {
                        "—".to_string()
                    } else {
                        self.settings.version.clone()
                    })
                    .width(240.0)
                    .show_ui(ui, |ui| {
                        for name in self.versions.clone() {
                            if ui
                                .selectable_value(&mut self.settings.version, name.clone(), &name)
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
                if ui.button("Actualiser").clicked() {
                    self.refresh_versions();
                }
                if changed {
                    self.refresh_java_hint();
                }
                ui.label(egui::RichText::new(&self.java_hint).color(MUTED));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("hôte:port").color(MUTED));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.server)
                            .desired_width(180.0)
                            .hint_text("connexion directe"),
                    );
                    ui.label("Serveur");
                });
            });
            ui.add_space(6.0);
        });
    }

    fn settings_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("reglages")
            .exact_size(380.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.heading("Réglages anti-surcharge");
                ui.add_space(4.0);
                egui::Grid::new("grille")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("RAM par instance");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.xmx_mb)
                                .range(512..=16384)
                                .speed(64)
                                .suffix(" Mo"),
                        );
                        ui.end_row();

                        ui.label("Instances simultanées max");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.max_instances).range(1..=12),
                        );
                        ui.end_row();

                        ui.label("RAM à garder libre");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.reserve_mb)
                                .range(512..=32768)
                                .speed(128)
                                .suffix(" Mo"),
                        );
                        ui.end_row();

                        ui.label("Délai mini entre lancements");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.stagger_min_s)
                                .range(0..=120)
                                .suffix(" s"),
                        );
                        ui.end_row();

                        ui.label("Attente max de chargement");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.stagger_max_s)
                                .range(15..=600)
                                .suffix(" s"),
                        );
                        ui.end_row();

                        ui.label("Abandon si pas de place");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.wait_timeout_s)
                                .range(30..=3600)
                                .speed(10)
                                .suffix(" s"),
                        );
                        ui.end_row();

                        ui.label("Cœurs par instance");
                        ui.add(
                            egui::DragValue::new(&mut self.settings.cores_per_instance)
                                .range(0..=64),
                        )
                        .on_hover_text("0 = pas de restriction d'affinité");
                        ui.end_row();

                        ui.label("Priorité des processus");
                        egui::ComboBox::from_id_salt("priorite")
                            .selected_text(self.settings.priority.label())
                            .show_ui(ui, |ui| {
                                for p in Priority::ALL {
                                    ui.selectable_value(&mut self.settings.priority, p, p.label());
                                }
                            });
                        ui.end_row();

                        ui.label("Fenêtre");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut self.settings.width).range(320..=7680),
                            );
                            ui.label("x");
                            ui.add(
                                egui::DragValue::new(&mut self.settings.height).range(240..=4320),
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.checkbox(
                    &mut self.settings.low_settings,
                    "Réglages graphiques bas pour les instances neuves",
                );
                ui.checkbox(
                    &mut self.settings.share_mods,
                    "Partager mods / resourcepacks / config",
                );
                ui.checkbox(
                    &mut self.settings.add_server_entry,
                    "Ajouter le serveur à la liste multijoueur",
                );
                ui.checkbox(
                    &mut self.settings.ignore_ram_guard,
                    "Ignorer le garde-fou RAM (à tes risques)",
                );
                ui.horizontal(|ui| {
                    ui.label("Arguments JVM");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings.extra_jvm)
                            .desired_width(f32::INFINITY),
                    );
                });

                if !self.advice.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new(&self.advice).color(MUTED));
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Calculer un réglage sûr").clicked() {
                        self.autotune();
                    }
                    if ui.button("Enregistrer").clicked() {
                        self.save_all();
                        self.manager.log("réglages enregistrés");
                    }
                });
            });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("etat").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let (total, avail) = sys::memory_mb();
                let used = total.saturating_sub(avail);
                let fraction = if total > 0 {
                    used as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(200.0)
                        .text(format!("{used} / {total} Mo")),
                );
                let running = self.manager.shared.running_count();
                let pending = self.manager.shared.pending_count();
                ui.label(
                    egui::RichText::new(format!(
                        "{running} instance(s) en jeu · {pending} en file · réserve {} Mo",
                        self.settings.reserve_mb
                    ))
                    .color(MUTED),
                );
            });
            ui.add_space(4.0);
        });
    }

    fn journal_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("journal")
            .resizable(true)
            .default_size(150.0)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.strong("Journal");
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
                            ui.label(egui::RichText::new(line).monospace().size(11.0));
                        }
                    });
            });
    }

    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            StripBuilder::new(ui)
                .size(Size::relative(0.55))
                .size(Size::remainder())
                .vertical(|mut strip| {
                    strip.cell(|ui| self.accounts_section(ui));
                    strip.cell(|ui| self.instances_section(ui));
                });
        });
    }

    fn accounts_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Comptes");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Tout décocher").clicked() {
                    self.accounts.iter_mut().for_each(|a| a.selected = false);
                }
                if ui.button("Tout cocher").clicked() {
                    self.accounts.iter_mut().for_each(|a| a.selected = true);
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Ajouter…").clicked() {
                self.dialog = Dialog::Account(AccountForm {
                    original: None,
                    name: String::new(),
                    version: String::new(),
                    xmx: String::new(),
                    instance: String::new(),
                    premium: false,
                    session_left: 0,
                    error: String::new(),
                });
            }
            if ui.button("Ajouter en lot…").clicked() {
                self.dialog = Dialog::Bulk(BulkForm {
                    text: String::new(),
                    prefix: "Alt".into(),
                    count: 4,
                });
            }
            if ui.button("Connexion Microsoft…").clicked() {
                self.dialog = Dialog::Microsoft(MsForm::new(self.settings.azure_client_id.clone()));
            }
            if ui.button("Importer du launcher").clicked() {
                self.import_official();
            }
            let has_selection = self.selected_account.is_some();
            if ui
                .add_enabled(has_selection, egui::Button::new("Modifier…"))
                .clicked()
                && let Some(name) = self.selected_account.clone()
                && let Some(account) = self.accounts.iter().find(|a| a.name == name)
            {
                self.dialog = Dialog::Account(AccountForm {
                    original: Some(account.name.clone()),
                    name: account.name.clone(),
                    version: account.version.clone().unwrap_or_default(),
                    xmx: account.xmx_mb.map(|v| v.to_string()).unwrap_or_default(),
                    instance: account.instance.clone(),
                    premium: account.is_premium(),
                    session_left: account.session_left(),
                    error: String::new(),
                });
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("Supprimer"))
                .clicked()
                && let Some(name) = self.selected_account.clone()
            {
                self.accounts.retain(|a| a.name != name);
                self.selected_account = None;
                let _ = save_accounts(&self.accounts);
            }
            if ui
                .add_enabled(has_selection, egui::Button::new("Ouvrir le dossier"))
                .clicked()
                && let Some(name) = self.selected_account.clone()
            {
                let settings = self.settings.clone();
                if let Some(account) = self.accounts.iter().find(|a| a.name == name) {
                    let dir = account.game_dir(&settings);
                    let _ = std::fs::create_dir_all(&dir);
                    Self::open_path(&dir);
                }
            }
        });
        ui.add_space(4.0);

        let row_height = 24.0;
        let mut to_select: Option<String> = None;
        TableBuilder::new(ui)
            .id_salt("comptes")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(26.0))
            .column(Column::remainder().at_least(120.0))
            .column(Column::exact(86.0))
            .column(Column::exact(150.0))
            .column(Column::exact(80.0))
            .header(20.0, |mut header| {
                header.col(|_ui| {});
                header.col(|ui| {
                    ui.strong("Pseudo");
                });
                header.col(|ui| {
                    ui.strong("Type");
                });
                header.col(|ui| {
                    ui.strong("Version");
                });
                header.col(|ui| {
                    ui.strong("RAM");
                });
            })
            .body(|body| {
                let selected = self.selected_account.clone();
                let default_version = self.settings.version.clone();
                body.rows(row_height, self.accounts.len(), |mut row| {
                    let index = row.index();
                    let account = &mut self.accounts[index];
                    row.col(|ui| {
                        ui.checkbox(&mut account.selected, "");
                    });
                    row.col(|ui| {
                        let is_selected = selected.as_deref() == Some(account.name.as_str());
                        if ui.selectable_label(is_selected, &account.name).clicked() {
                            to_select = Some(account.name.clone());
                        }
                    });
                    row.col(|ui| {
                        let color = if account.is_premium() { ACCENT } else { MUTED };
                        ui.label(egui::RichText::new(account.kind.label()).color(color));
                    });
                    row.col(|ui| {
                        let text = account
                            .version
                            .clone()
                            .unwrap_or_else(|| format!("({default_version})"));
                        ui.label(egui::RichText::new(text).color(MUTED));
                    });
                    row.col(|ui| {
                        let text = account
                            .xmx_mb
                            .map(|v| format!("{v} Mo"))
                            .unwrap_or_else(|| "(globale)".into());
                        ui.label(egui::RichText::new(text).color(MUTED));
                    });
                });
            });
        if to_select.is_some() {
            self.selected_account = to_select;
        }
    }

    fn instances_section(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Lancer la sélection")
                            .strong()
                            .size(14.0),
                    )
                    .fill(egui::Color32::from_rgb(56, 108, 58)),
                )
                .clicked()
            {
                self.launch_selected();
            }
            if ui.button("Vider la file").clicked() {
                self.manager.clear_queue();
            }
            let has_instance = self.selected_instance.is_some();
            if ui
                .add_enabled(has_instance, egui::Button::new("Arrêter"))
                .clicked()
                && let Some(id) = self.selected_instance
            {
                self.manager.kill(id);
            }
            if ui
                .add(egui::Button::new("Tout arrêter").fill(egui::Color32::from_rgb(120, 52, 52)))
                .clicked()
            {
                self.manager.clear_queue();
                self.manager.kill_all();
            }
            if ui
                .add_enabled(has_instance, egui::Button::new("Voir le log"))
                .clicked()
                && let Some(id) = self.selected_instance
            {
                let path = self.manager.shared.instances.lock().ok().and_then(|list| {
                    list.iter()
                        .find(|i| i.id == id)
                        .and_then(|i| i.log_path.clone())
                });
                match path {
                    Some(path) if path.is_file() => Self::open_path(&path),
                    _ => self.manager.log("aucun log pour cette ligne"),
                }
            }
            if ui.button("Nettoyer").clicked() {
                self.manager.forget_finished();
            }
        });
        ui.add_space(4.0);

        #[derive(Clone)]
        struct Row {
            id: u64,
            account: String,
            version: String,
            state: State,
            rss: u64,
            uptime: String,
            pid: String,
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
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut clicked: Option<u64> = None;
        TableBuilder::new(ui)
            .id_salt("instances")
            .striped(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::exact(30.0))
            .column(Column::remainder().at_least(100.0))
            .column(Column::exact(150.0))
            .column(Column::exact(90.0))
            .column(Column::exact(70.0))
            .column(Column::exact(70.0))
            .column(Column::exact(60.0))
            .header(20.0, |mut header| {
                for title in ["#", "Pseudo", "Version", "État", "RAM", "Durée", "PID"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                let selected = self.selected_instance;
                body.rows(24.0, rows.len(), |mut row| {
                    let item = &rows[row.index()];
                    row.col(|ui| {
                        ui.label(egui::RichText::new(item.id.to_string()).color(MUTED));
                    });
                    row.col(|ui| {
                        if ui
                            .selectable_label(selected == Some(item.id), &item.account)
                            .clicked()
                        {
                            clicked = Some(item.id);
                        }
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&item.version).color(MUTED));
                    });
                    row.col(|ui| {
                        let color = match item.state {
                            State::Running => ACCENT,
                            State::Crashed => BAD,
                            State::Queued | State::WaitingRoom | State::Starting => WARN,
                            _ => MUTED,
                        };
                        ui.label(egui::RichText::new(item.state.label()).color(color));
                    });
                    row.col(|ui| {
                        if item.rss > 0 {
                            ui.label(format!("{} Mo", item.rss));
                        }
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&item.uptime).color(MUTED));
                    });
                    row.col(|ui| {
                        ui.label(egui::RichText::new(&item.pid).color(MUTED));
                    });
                });
            });
        if clicked.is_some() {
            self.selected_instance = clicked;
        }
    }

    /// Reprend pseudos et UUID des comptes du launcher officiel.
    fn import_official(&mut self) {
        let path = self.settings.mc_dir.join("launcher_accounts.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            self.manager.log(format!("{} illisible", path.display()));
            return;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            self.manager.log("launcher_accounts.json invalide");
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
        self.manager.log(format!(
            "import du launcher officiel : {added} compte(s) — pseudo et UUID repris, \
             session hors-ligne"
        ));
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut close = false;
        match &mut self.dialog {
            Dialog::None => {}
            Dialog::Account(_) => close = self.account_dialog(ctx),
            Dialog::Bulk(_) => close = self.bulk_dialog(ctx),
            Dialog::Microsoft(_) => close = self.microsoft_dialog(ctx),
        }
        if close {
            self.dialog = Dialog::None;
        }
    }

    fn account_dialog(&mut self, ctx: &egui::Context) -> bool {
        let Dialog::Account(form) = &mut self.dialog else {
            return false;
        };
        let versions = self.versions.clone();
        let mut close = false;
        let mut commit = false;
        let mut open = true;
        egui::Window::new(if form.original.is_some() {
            "Modifier le compte"
        } else {
            "Nouveau compte"
        })
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Grid::new("compte")
                .num_columns(2)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Pseudo");
                    ui.add_enabled(
                        !form.premium,
                        egui::TextEdit::singleline(&mut form.name).desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Version");
                    egui::ComboBox::from_id_salt("version-compte")
                        .selected_text(if form.version.is_empty() {
                            "(celle du haut)".to_string()
                        } else {
                            form.version.clone()
                        })
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut form.version,
                                String::new(),
                                "(celle du haut)",
                            );
                            for name in &versions {
                                ui.selectable_value(&mut form.version, name.clone(), name);
                            }
                        });
                    ui.end_row();

                    ui.label("RAM (Mo)");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.xmx)
                            .desired_width(220.0)
                            .hint_text("vide = réglage global"),
                    );
                    ui.end_row();

                    ui.label("Dossier d'instance");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.instance)
                            .desired_width(220.0)
                            .hint_text("vide = d'après le pseudo"),
                    );
                    ui.end_row();
                });

            ui.add_space(6.0);
            let note = if form.premium {
                format!(
                    "Compte Microsoft : pseudo et UUID viennent de Mojang.\n\
                     Session valide encore {} min ; elle se renouvelle au lancement.",
                    (form.session_left / 60).max(0)
                )
            } else {
                "Compte hors-ligne : l'UUID est calculé comme le fait un serveur en \
                 online-mode=false.\nPour un serveur premium, passe par Connexion Microsoft."
                    .to_string()
            };
            ui.label(egui::RichText::new(note).color(MUTED));
            if !form.error.is_empty() {
                ui.label(egui::RichText::new(&form.error).color(BAD));
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Valider").clicked() {
                    commit = true;
                }
                if ui.button("Annuler").clicked() {
                    close = true;
                }
            });
        });

        if commit {
            let name = form.name.trim().to_string();
            if name.is_empty() {
                form.error = "Le pseudo est obligatoire.".into();
                return false;
            }
            let duplicate = self
                .accounts
                .iter()
                .any(|a| a.name == name && Some(&a.name) != form.original.as_ref());
            if duplicate {
                form.error = "Ce pseudo existe déjà.".into();
                return false;
            }
            let version = (!form.version.is_empty()).then(|| form.version.clone());
            let xmx = form.xmx.trim().parse::<u64>().ok();
            let instance = if form.instance.trim().is_empty() {
                sanitize(&name)
            } else {
                form.instance.trim().to_string()
            };
            match form.original.clone() {
                Some(original) => {
                    let premium = form.premium;
                    if let Some(account) = self.account_mut(&original) {
                        if !premium {
                            account.name = name.clone();
                            account.uuid = crate::auth::offline_uuid(&name);
                        }
                        account.version = version;
                        account.xmx_mb = xmx;
                        account.instance = instance;
                    }
                    if self.selected_account.as_deref() == Some(original.as_str()) {
                        self.selected_account = Some(name);
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
        let Dialog::Bulk(form) = &mut self.dialog else {
            return false;
        };
        let mut close = false;
        let mut commit = false;
        let mut open = true;
        egui::Window::new("Ajouter plusieurs comptes")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Un pseudo par ligne :");
                ui.add(
                    egui::TextEdit::multiline(&mut form.text)
                        .desired_width(280.0)
                        .desired_rows(10),
                );
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("ou générer");
                    ui.add(egui::TextEdit::singleline(&mut form.prefix).desired_width(80.0));
                    ui.add(egui::DragValue::new(&mut form.count).range(1..=32));
                    if ui.button("Remplir").clicked() {
                        form.text = (1..=form.count)
                            .map(|i| format!("{}{i}", form.prefix.trim()))
                            .collect::<Vec<_>>()
                            .join("\n");
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Ajouter").clicked() {
                        commit = true;
                    }
                    if ui.button("Annuler").clicked() {
                        close = true;
                    }
                });
            });

        if commit {
            let names: Vec<String> = form
                .text
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && l.len() <= 16)
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
            self.manager.log(format!("{added} compte(s) ajouté(s)"));
            return true;
        }
        close || !open
    }

    fn microsoft_dialog(&mut self, ctx: &egui::Context) -> bool {
        let Dialog::Microsoft(form) = &mut self.dialog else {
            return false;
        };
        let mut close = false;
        let mut open = true;
        let mut start = false;
        let mut finished: Option<Account> = None;

        egui::Window::new("Connexion Microsoft")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Identifiant d'application Azure");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.client_id)
                            .desired_width(260.0)
                            .hint_text("ID d'application (client)"),
                    );
                })
                .response
                .on_hover_text(AZURE_HELP);

                ui.add_space(10.0);
                let code = form
                    .flow
                    .lock()
                    .ok()
                    .and_then(|f| f.as_ref().map(|f| f.user_code.clone()));
                let display = code.clone().unwrap_or_else(|| "—".into());
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(display)
                            .monospace()
                            .size(30.0)
                            .color(ACCENT),
                    );
                });
                ui.add_space(6.0);

                let status = form.status.lock().map(|s| s.clone()).unwrap_or_default();
                ui.label(egui::RichText::new(status).color(MUTED));

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!form.running, egui::Button::new("Obtenir un code"))
                        .clicked()
                    {
                        start = true;
                    }
                    if ui
                        .add_enabled(code.is_some(), egui::Button::new("Ouvrir la page"))
                        .clicked()
                        && let Ok(flow) = form.flow.lock()
                        && let Some(flow) = flow.as_ref()
                    {
                        ctx.open_url(egui::OpenUrl::new_tab(&flow.verification_uri));
                    }
                    if ui
                        .add_enabled(code.is_some(), egui::Button::new("Copier le code"))
                        .clicked()
                        && let Some(code) = code.clone()
                    {
                        ctx.copy_text(code);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Fermer").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.label(egui::RichText::new(AZURE_HELP).color(MUTED).size(11.0));
            });

        if start {
            let client_id = form.client_id.trim().to_string();
            if client_id.is_empty() {
                if let Ok(mut status) = form.status.lock() {
                    *status = "Renseigne d'abord l'identifiant d'application Azure.".into();
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
                if let Ok(mut s) = status.lock() {
                    *s = "Demande du code à Microsoft…".into();
                }
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let status_code = Arc::clone(&status);
                    let repaint_code = repaint.clone();
                    let status_wait = Arc::clone(&status);
                    let repaint_wait = repaint.clone();
                    let stop_flag = Arc::clone(&stop);
                    let result = msa::login_device(
                        &client_id,
                        |device| {
                            if let Ok(mut slot) = flow.lock() {
                                *slot = Some(device.clone());
                            }
                            if let Ok(mut s) = status_code.lock() {
                                *s = format!(
                                    "Ouvre {} et saisis le code ci-dessus, puis connecte-toi \
                                     avec le compte premium.",
                                    device.verification_uri
                                );
                            }
                            repaint_code.request_repaint();
                        },
                        move || stop_flag.load(Ordering::Relaxed),
                        |left| {
                            if let Ok(mut s) = status_wait.lock() {
                                *s = format!(
                                    "En attente de la validation… ({} min restantes)",
                                    left / 60
                                );
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

        // Resultat du thread de connexion.
        let outcome = form.outcome.lock().ok().and_then(|mut slot| slot.take());
        match outcome {
            Some(Ok(account)) => finished = Some(account),
            Some(Err(message)) => {
                form.running = false;
                if let Ok(mut status) = form.status.lock() {
                    *status = format!("Échec : {message}");
                }
                self.manager
                    .log(format!("connexion Microsoft échouée : {message}"));
            }
            None => {}
        }

        if let Some(account) = finished {
            self.manager.log(format!(
                "[{}] compte premium connecté (UUID {})",
                account.name, account.uuid
            ));
            self.upsert(account);
            return true;
        }
        if close || !open {
            form.stop.store(true, Ordering::Relaxed);
            return true;
        }
        false
    }
}

const AZURE_HELP: &str = "\
Une application Azure personnelle est nécessaire (gratuite) :
1. portal.azure.com > Microsoft Entra ID > Inscriptions d'applications > Nouvelle inscription
2. comptes pris en charge : comptes Microsoft personnels uniquement, sans URI de redirection
3. Authentification > Autoriser les flux client publics = Oui
4. copier l'ID d'application (client) ici";

/// Theme sombre, un peu plus contraste que celui d'origine.
pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(30, 31, 34);
    visuals.window_fill = egui::Color32::from_rgb(38, 40, 44);
    visuals.extreme_bg_color = egui::Color32::from_rgb(24, 25, 28);
    visuals.selection.bg_fill = egui::Color32::from_rgb(60, 92, 62);
    ctx.set_visuals(visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
    });
}

#[cfg(test)]
mod tests {
    use crate::config::AccountKind;

    #[test]
    fn account_kind_labels_are_distinct() {
        assert_ne!(AccountKind::Offline.label(), AccountKind::Microsoft.label());
    }
}
