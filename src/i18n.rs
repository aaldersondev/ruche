//! Traductions.
//!
//! Chaque texte est déclaré une fois avec ses deux versions côte à côte ; la
//! macro en fait une structure de `&'static str`, donc un oubli de traduction
//! ne compile pas. Les emplacements variables s'écrivent `{0}`, `{1}`… et sont
//! remplis par [`fill`].

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Fr,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Fr, Lang::En];

    /// Nom de la langue, écrit dans cette langue.
    pub fn label(self) -> &'static str {
        match self {
            Lang::Fr => "Français",
            Lang::En => "English",
        }
    }

    pub fn strings(self) -> &'static Strings {
        match self {
            Lang::Fr => &FR,
            Lang::En => &EN,
        }
    }
}

/// Remplace `{0}`, `{1}`… par les valeurs fournies.
pub fn fill(template: &str, args: &[&str]) -> String {
    let mut out = template.to_string();
    for (index, value) in args.iter().enumerate() {
        out = out.replace(&format!("{{{index}}}"), value);
    }
    out
}

macro_rules! strings {
    ($($key:ident => $fr:expr, $en:expr;)*) => {
        /// Tous les textes visibles de l'application.
        #[allow(missing_docs)]
        pub struct Strings { $(pub $key: &'static str,)* }

        pub static FR: Strings = Strings { $($key: $fr,)* };
        pub static EN: Strings = Strings { $($key: $en,)* };
    };
}

strings! {
    // ---------------------------------------------------------- navigation
    tab_accounts => "Comptes", "Accounts";
    tab_instances => "Instances", "Instances";
    tab_settings => "Réglages", "Settings";
    tagline => "launcher multi-comptes", "multi-account launcher";

    // ------------------------------------------------------ bandeau de lancement
    version => "Version", "Version";
    refresh => "Actualiser", "Refresh";
    no_version => "aucune version installée", "no version installed";
    server => "Serveur", "Server";
    server_hint => "hôte:port — connexion directe", "host:port — join on launch";
    java_line => "Java {0} · {1}", "Java {0} · {1}";
    java_missing => "JRE introuvable", "JRE not found";
    inherits => "hérite de {0}", "inherits from {0}";
    launch => "Lancer", "Play";
    launch_count => "{0} compte(s) coché(s)", "{0} account(s) checked";
    launch_none => "aucun compte coché", "no account checked";

    // ------------------------------------------------------------- comptes
    add_account => "Ajouter", "Add";
    add_bulk => "Ajouter en lot", "Add in bulk";
    add_microsoft => "Connexion Microsoft", "Microsoft sign-in";
    import_launcher => "Importer du launcher", "Import from launcher";
    select_all => "Tout cocher", "Check all";
    deselect_all => "Tout décocher", "Uncheck all";
    kind_offline => "hors-ligne", "offline";
    kind_premium => "premium", "premium";
    edit => "Modifier", "Edit";
    delete => "Supprimer", "Delete";
    open_folder => "Dossier", "Folder";
    chip_global => "version globale", "global version";
    chip_ram_global => "RAM globale", "global RAM";
    session_left => "session {0} min", "session {0} min";
    session_left_hours => "session {0} h", "session {0} h";
    session_expired => "session à renouveler", "session needs a refresh";
    no_accounts_title => "Aucun compte", "No accounts yet";
    no_accounts_hint =>
        "Ajoute un pseudo hors-ligne, génères-en plusieurs d'un coup, ou connecte un compte Microsoft.",
        "Add an offline name, generate a batch of them, or sign in with a Microsoft account.";

    // ----------------------------------------------------------- instances
    clear_queue => "Vider la file", "Clear queue";
    stop => "Arrêter", "Stop";
    stop_all => "Tout arrêter", "Stop all";
    view_log => "Log", "Log";
    cleanup => "Nettoyer", "Clean up";
    no_instances_title => "Rien en cours", "Nothing running";
    no_instances_hint =>
        "Coche des comptes dans l'onglet Comptes, puis lance-les. Ils apparaîtront ici avec leur mémoire et leur durée.",
        "Check a few accounts in the Accounts tab and start them. They will show up here with their memory use and uptime.";
    col_ram => "RAM", "RAM";
    col_uptime => "Durée", "Uptime";
    state_queued => "en file", "queued";
    state_waiting => "attente RAM", "waiting for RAM";
    state_starting => "démarrage", "starting";
    state_running => "en jeu", "running";
    state_stopped => "arrêté", "stopped";
    state_crashed => "échec", "failed";
    state_aborted => "abandonné", "given up";

    // ------------------------------------------------------------ réglages
    sec_memory => "Mémoire", "Memory";
    sec_pace => "Cadence de lancement", "Launch pacing";
    sec_system => "Système", "System";
    sec_game => "Jeu", "Game";
    sec_premium => "Comptes premium", "Premium accounts";
    sec_appearance => "Langue et dossiers", "Language and folders";

    set_xmx => "RAM par instance", "RAM per instance";
    set_xmx_hint =>
        "Le plafond du tas Java. 2 Go suffisent en vanilla ; monte pour des gros packs de mods.",
        "The Java heap cap. 2 GB is plenty for vanilla; raise it for heavy mod packs.";
    set_reserve => "RAM à garder libre", "RAM to keep free";
    set_reserve_hint =>
        "Ce que le launcher refuse d'entamer, pour que Windows et le reste de tes applications respirent.",
        "What the launcher refuses to eat into, so Windows and your other apps keep breathing.";
    set_max => "Instances simultanées", "Simultaneous instances";
    set_stagger_min => "Délai mini entre deux lancements", "Minimum delay between launches";
    set_stagger_max => "Attente max de chargement", "Maximum wait for loading";
    set_stagger_hint =>
        "La file attend que le client précédent ait ouvert sa fenêtre avant d'enchaîner, sans jamais dépasser ce délai.",
        "The queue waits for the previous client to open its window before moving on, never longer than this.";
    set_timeout => "Abandon si pas de place après", "Give up if no room after";
    set_priority => "Priorité des processus", "Process priority";
    set_cores => "Cœurs par instance", "Cores per instance";
    set_cores_hint => "0 = pas de restriction d'affinité", "0 = no affinity restriction";
    set_ignore_guard => "Ignorer le garde-fou RAM", "Ignore the RAM guard";
    set_ignore_guard_hint =>
        "À tes risques : les instances partiront même si la mémoire manque.",
        "At your own risk: instances will start even when memory is short.";
    set_window => "Taille de la fenêtre", "Window size";
    set_fullscreen => "Plein écran", "Fullscreen";
    set_low => "Réglages graphiques bas pour les instances neuves",
               "Low graphics defaults for fresh instances";
    set_share => "Partager mods, resourcepacks et config avec .minecraft",
                 "Share mods, resource packs and config with .minecraft";
    set_add_server => "Ajouter le serveur à la liste multijoueur",
                      "Add the server to the multiplayer list";
    set_extra_jvm => "Arguments JVM supplémentaires", "Extra JVM arguments";
    set_azure => "Identifiant d'application Azure", "Azure application ID";
    set_language => "Langue", "Language";
    set_mc_dir => "Dossier .minecraft", "Minecraft folder";
    set_instances_dir => "Dossier des instances", "Instances folder";
    open => "Ouvrir", "Open";
    autotune => "Calculer un réglage sûr", "Suggest safe settings";
    save => "Enregistrer", "Save";
    prio_normal => "normale", "normal";
    prio_below => "basse", "below normal";
    prio_idle => "inactive", "idle";
    unit_mb => " Mo", " MB";
    unit_s => " s", " s";

    // ------------------------------------------------------------- discord
    sec_discord => "Discord", "Discord";
    set_discord => "Afficher une activité Discord", "Show a Discord activity";
    set_discord_hint =>
        "Tes contacts voient le nombre de clients ouverts et la version, jamais les pseudos.",
        "Your friends see how many clients are open and which version, never the account names.";
    set_discord_app => "Identifiant d'application Discord", "Discord application ID";
    discord_off => "désactivé", "off";
    discord_connecting => "connexion…", "connecting…";
    discord_connected => "connecté", "connected";
    discord_unavailable => "indisponible : {0}", "unavailable: {0}";
    discord_help =>
        "Il faut une application Discord (gratuite) :\n1. discord.com/developers/applications > New Application\n2. copier l'Application ID ici\n3. facultatif : Rich Presence > Art Assets, y déposer une image nommée « ruche »\nDiscord doit tourner sur la même machine.",
        "You need a Discord application (free):\n1. discord.com/developers/applications > New Application\n2. copy the Application ID here\n3. optional: Rich Presence > Art Assets, upload an image named \"ruche\"\nDiscord must be running on the same machine.";
    rp_idle => "Au repos", "Idle";
    rp_running => "{0} client(s) en jeu", "{0} client(s) running";
    rp_queued => "{0} en file d'attente", "{0} waiting in the queue";
    rp_on_version => "sur {0}", "on {0}";

    // ------------------------------------------------------------- statut
    status_ram => "RAM {0} / {1} Mo", "RAM {0} / {1} MB";
    status_counts => "{0} en jeu · {1} en file · réserve {2} Mo",
                     "{0} running · {1} queued · {2} MB reserved";
    show_log => "Journal", "Log";

    // ----------------------------------------------------------- dialogues
    dlg_new_account => "Nouveau compte", "New account";
    dlg_edit_account => "Modifier le compte", "Edit account";
    field_name => "Pseudo", "Name";
    field_version => "Version", "Version";
    field_version_default => "(celle du bandeau)", "(the one on the bar)";
    field_ram => "RAM", "RAM";
    field_ram_hint => "vide = réglage global", "empty = global setting";
    field_instance => "Dossier d'instance", "Instance folder";
    field_instance_hint => "vide = d'après le pseudo", "empty = derived from the name";
    note_offline =>
        "Compte hors-ligne : l'UUID est calculé comme le fait un serveur en online-mode=false. Pour un serveur premium, passe par la connexion Microsoft.",
        "Offline account: the UUID is derived exactly as an online-mode=false server does. For a premium server, use the Microsoft sign-in instead.";
    note_premium =>
        "Compte Microsoft : le pseudo et l'UUID viennent de Mojang. La session est renouvelée toute seule au lancement.",
        "Microsoft account: name and UUID come from Mojang. The session is refreshed automatically at launch.";
    validate => "Valider", "Confirm";
    cancel => "Annuler", "Cancel";
    err_name_required => "Le pseudo est obligatoire.", "A name is required.";
    err_name_exists => "Ce pseudo existe déjà.", "That name is already taken.";
    dlg_bulk => "Ajouter plusieurs comptes", "Add several accounts";
    bulk_lines => "Un pseudo par ligne :", "One name per line:";
    bulk_generate => "ou générer", "or generate";
    bulk_fill => "Remplir", "Fill in";
    add => "Ajouter", "Add";
    dlg_microsoft => "Connexion Microsoft", "Microsoft sign-in";
    ms_intro =>
        "Colle ton identifiant d'application Azure, puis demande un code.",
        "Paste your Azure application ID, then ask for a code.";
    ms_asking => "Demande du code à Microsoft…", "Asking Microsoft for a code…";
    ms_code_hint =>
        "Le code est dans le presse-papier. Ouvre {0}, colle-le, et connecte-toi avec le compte premium.",
        "The code is in your clipboard. Open {0}, paste it, and sign in with the premium account.";
    ms_waiting => "En attente de la validation… ({0} min restantes)",
                  "Waiting for you to confirm… ({0} min left)";
    ms_get_code => "Obtenir un code", "Get a code";
    ms_open_page => "Ouvrir la page", "Open the page";
    ms_copy => "Copier le code", "Copy the code";
    close => "Fermer", "Close";
    ms_need_id => "Renseigne d'abord l'identifiant d'application Azure.",
                  "Fill in the Azure application ID first.";
    ms_failed => "Échec : {0}", "Failed: {0}";
    azure_help =>
        "Une application Azure personnelle est nécessaire (gratuite) :\n1. portal.azure.com > Microsoft Entra ID > Inscriptions d'applications > Nouvelle inscription\n2. comptes pris en charge : comptes Microsoft personnels uniquement, sans URI de redirection\n3. Authentification > Autoriser les flux client publics = Oui\n4. copier l'ID d'application (client) ici",
        "You need your own Azure application (free):\n1. portal.azure.com > Microsoft Entra ID > App registrations > New registration\n2. supported account types: personal Microsoft accounts only, no redirect URI\n3. Authentication > Allow public client flows = Yes\n4. copy the Application (client) ID here";

    // ------------------------------------------------------------ journal
    log_versions_found => "{0} version(s) trouvée(s) dans {1}", "{0} version(s) found in {1}";
    log_ram_summary => "RAM {0} Mo libres sur {1} — {2} instance(s) de {3} Mo tiennent tout de suite",
                       "{0} MB free out of {1} — room for {2} instance(s) of {3} MB right now";
    log_settings_saved => "réglages enregistrés", "settings saved";
    log_no_account => "aucun compte coché", "no account checked";
    log_no_version => "aucune version sélectionnée", "no version selected";
    log_room_warning =>
        "{0} Mo libres : {1} instance(s) partent tout de suite, les autres attendent qu'il y ait de la place",
        "{0} MB free: {1} instance(s) start right away, the rest wait for room";
    log_imported => "import du launcher officiel : {0} compte(s) — pseudo et UUID repris, session hors-ligne",
                    "imported from the official launcher: {0} account(s) — name and UUID reused, offline session";
    log_import_failed => "{0} illisible", "{0} could not be read";
    log_added => "{0} compte(s) ajouté(s)", "{0} account(s) added";
    log_no_log => "aucun log pour cette instance", "no log for that instance";
    log_connected => "[{0}] compte premium connecté (UUID {1})",
                     "[{0}] premium account connected (UUID {1})";
    log_connect_failed => "connexion Microsoft échouée : {0}", "Microsoft sign-in failed: {0}";
    log_queued => "[{0}] mis en file sur {1}", "[{0}] queued on {1}";
    log_cap_reached => "[{0}] plafond de {1} instances atteint, en attente",
                       "[{0}] cap of {1} instances reached, waiting";
    log_ram_short =>
        "[{0}] RAM insuffisante ({1} Mo libres, il en faut {2} + {3} Mo de réserve) — en attente",
        "[{0}] not enough RAM ({1} MB free, {2} needed plus {3} MB reserved) — waiting";
    log_gave_up =>
        "[{0}] abandonné : toujours pas de place après {1} s (baisse la RAM par instance, ferme des applications, ou désactive le garde-fou)",
        "[{0}] gave up: still no room after {1} s (lower the per-instance RAM, close some apps, or turn the guard off)";
    log_downloading => "téléchargement de {0}", "downloading {0}";
    log_missing_files => "[{0}] {1} fichier(s) manquant(s), tentative de téléchargement",
                         "[{0}] {1} file(s) missing, trying to download them";
    log_missing_failed => "fichiers introuvables : {0}", "files not found: {0}";
    log_launching => "[{0}] lancement de {1} ({2} Mo, {3})", "[{0}] starting {1} ({2} MB, {3})";
    log_loaded => "[{0}] client chargé (pid {1})", "[{0}] client loaded (pid {1})";
    log_still_loading => "[{0}] toujours en chargement après {1} s, on enchaîne",
                         "[{0}] still loading after {1} s, moving on";
    log_finished => "[{0}] terminé (code {1}){2}", "[{0}] exited (code {1}){2}";
    log_see => " — voir {0}", " — see {0}";
    log_stop_requested => "[{0}] arrêt demandé", "[{0}] stop requested";
    log_queue_cleared => "file d'attente vidée ({0} instance(s))", "queue cleared ({0} instance(s))";
    log_java_failed => "java n'a pas démarré : {0}", "java did not start: {0}";
    log_instance_dir => "dossier d'instance : {0}", "instance folder: {0}";
    log_premium_error => "compte Microsoft — {0}", "Microsoft account — {0}";

    // -------------------------------------------------------------- auth
    auth_no_client_id => "aucun identifiant d'application Azure n'est configuré",
                         "no Azure application ID is configured";
    auth_cancelled => "connexion annulée", "sign-in cancelled";
    auth_code_expired => "le code a expiré, recommence", "the code expired, start again";
    auth_declined => "connexion refusée sur la page Microsoft", "sign-in declined on the Microsoft page";
    auth_timeout => "délai dépassé : le code n'a pas été validé", "timed out: the code was never confirmed";
    auth_session_expired => "session Microsoft expirée ({0}) — reconnecte le compte",
                            "Microsoft session expired ({0}) — sign in again";
    auth_xbox_refused => "Xbox Live a refusé la session Microsoft", "Xbox Live refused the Microsoft session";
    auth_no_xbox_profile => "ce compte Microsoft n'a pas de profil Xbox : crée-le sur xbox.com",
                            "this Microsoft account has no Xbox profile: create one on xbox.com";
    auth_country => "Xbox Live n'est pas disponible dans le pays du compte",
                    "Xbox Live is not available in this account's country";
    auth_adult => "le compte doit passer une vérification adulte", "the account needs adult verification";
    auth_child => "compte enfant : il doit être rattaché à un groupe familial",
                  "child account: it must belong to a family group";
    auth_xsts => "XSTS a refusé le compte (code {0})", "XSTS refused the account (code {0})";
    auth_xsts_no_token => "XSTS n'a pas renvoyé de jeton", "XSTS returned no token";
    auth_mc_refused => "Minecraft Services a refusé la connexion (HTTP {0})",
                       "Minecraft Services refused the sign-in (HTTP {0})";
    auth_no_profile => "aucun profil : choisis d'abord un pseudo sur minecraft.net",
                       "no profile: pick a name on minecraft.net first";
    auth_not_owned => "ce compte ne possède pas Minecraft Java Edition",
                      "this account does not own Minecraft Java Edition";
    auth_profile_unreadable => "profil Minecraft illisible (HTTP {0})",
                               "Minecraft profile could not be read (HTTP {0})";
    auth_no_refresh => "aucun jeton de rafraîchissement : reconnecte {0}",
                       "no refresh token: sign in again for {0}";
    auth_refreshing => "[{0}] session Microsoft expirée, renouvellement",
                       "[{0}] Microsoft session expired, refreshing";
    auth_network => "réseau injoignable : {0}", "network unreachable: {0}";
    auth_unreadable => "réponse illisible : {0}", "response could not be read: {0}";
    auth_unexpected => "réponse inattendue : {0}", "unexpected response: {0}";
    auth_ms_unknown => "réponse Microsoft incomprise", "unrecognised Microsoft response";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_filled_in_order() {
        assert_eq!(fill("{0} sur {1}", &["3", "7"]), "3 sur 7");
        assert_eq!(fill("rien à remplacer", &[]), "rien à remplacer");
        // un argument en trop ne casse rien
        assert_eq!(fill("{0}", &["a", "b"]), "a");
    }

    #[test]
    fn both_languages_answer() {
        for lang in Lang::ALL {
            let s = lang.strings();
            assert!(!s.tab_accounts.is_empty());
            assert!(!s.launch.is_empty());
            assert!(!s.auth_no_client_id.is_empty());
        }
        assert_ne!(FR.tab_settings, EN.tab_settings);
    }

    #[test]
    fn every_placeholder_survives_translation() {
        // Les deux versions d'un meme texte doivent attendre les memes arguments.
        let pairs: [(&str, &str); 8] = [
            (FR.log_ram_short, EN.log_ram_short),
            (FR.log_launching, EN.log_launching),
            (FR.log_finished, EN.log_finished),
            (FR.log_ram_summary, EN.log_ram_summary),
            (FR.status_counts, EN.status_counts),
            (FR.ms_waiting, EN.ms_waiting),
            (FR.auth_xsts, EN.auth_xsts),
            (FR.log_connected, EN.log_connected),
        ];
        for (fr, en) in pairs {
            for index in 0..4 {
                let token = format!("{{{index}}}");
                assert_eq!(
                    fr.contains(&token),
                    en.contains(&token),
                    "l'emplacement {token} manque d'un cote : {fr} / {en}"
                );
            }
        }
    }
}
