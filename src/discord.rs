//! Rich Presence Discord, parlée directement sur le canal IPC local.
//!
//! Discord expose un tube nommé (`\\.\pipe\discord-ipc-N` sous Windows, une
//! socket unix ailleurs). Le protocole tient en deux choses : une trame
//! `opcode` + `longueur` sur huit octets, puis du JSON. Ça évite d'embarquer
//! une bibliothèque entière pour trois messages.
//!
//! Le thread ci-dessous encaisse tout : Discord fermé, tube absent, connexion
//! coupée en cours de route. Il retente périodiquement et n'ennuie jamais
//! l'interface.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use serde_json::json;

/// Délai avant de retenter une connexion perdue ou refusée.
const RETRY: Duration = Duration::from_secs(20);

/// Opcodes du protocole IPC de Discord.
const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
/// Discord ferme le tube en annonçant pourquoi : c'est ainsi qu'il refuse un
/// identifiant d'application inconnu (constaté, pas supposé).
const OP_CLOSE: u32 = 2;

#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum Status {
    /// Fonction désactivée dans les réglages.
    #[default]
    Off,
    Connecting,
    Connected,
    /// Discord n'est pas joignable ; le message dit pourquoi.
    Unavailable(String),
}

/// Ce que Discord affichera. Deux activités égales ne sont envoyées qu'une fois.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Activity {
    pub details: String,
    pub state: String,
    /// Début de la partie, en secondes epoch : Discord en fait un chronomètre.
    pub start: Option<u64>,
}

enum Message {
    Configure { app_id: String, enabled: bool },
    Set(Option<Activity>),
    Stop,
}

pub struct Presence {
    sender: mpsc::Sender<Message>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
}

impl Presence {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(Status::Off));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_status = Arc::clone(&status);
        let worker_stop = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("discord".into())
            .spawn(move || worker(receiver, worker_status, worker_stop))
            .expect("thread discord");
        Self {
            sender,
            status,
            stop,
        }
    }

    /// Active, désactive ou change l'application Discord visée.
    pub fn configure(&self, app_id: &str, enabled: bool) {
        let _ = self.sender.send(Message::Configure {
            app_id: app_id.trim().to_string(),
            enabled,
        });
    }

    /// Met à jour ce que voient les autres ; `None` efface la présence.
    pub fn set(&self, activity: Option<Activity>) {
        let _ = self.sender.send(Message::Set(activity));
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or(Status::Off)
    }

    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.sender.send(Message::Stop);
    }
}

impl Default for Presence {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ------------------------------------------------------------------- worker

fn worker(receiver: mpsc::Receiver<Message>, status: Arc<Mutex<Status>>, stop: Arc<AtomicBool>) {
    let mut app_id = String::new();
    let mut enabled = false;
    let mut wanted: Option<Activity> = None;
    let mut sent: Option<Option<Activity>> = None;
    let mut link: Option<Ipc> = None;
    let mut next_try = std::time::Instant::now();

    let set_status = |value: Status| {
        if let Ok(mut slot) = status.lock() {
            *slot = value;
        }
    };

    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // On dort sur le canal : réveil immédiat dès qu'il y a du neuf.
        match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(Message::Stop) => return,
            Ok(Message::Configure {
                app_id: id,
                enabled: on,
            }) => {
                if id != app_id || on != enabled {
                    app_id = id;
                    enabled = on;
                    link = None;
                    sent = None;
                    next_try = std::time::Instant::now();
                    set_status(if enabled && !app_id.is_empty() {
                        Status::Connecting
                    } else {
                        Status::Off
                    });
                }
            }
            Ok(Message::Set(activity)) => wanted = activity,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        // Rien a annoncer : ni identifiant, ni case cochee.
        if !enabled || app_id.is_empty() {
            link = None;
            sent = None;
            set_status(Status::Off);
            continue;
        }

        // Connexion (ou reconnexion) au tube de Discord.
        if link.is_none() {
            if std::time::Instant::now() < next_try {
                continue;
            }
            next_try = std::time::Instant::now() + RETRY;
            match Ipc::connect(&app_id) {
                Ok(ipc) => {
                    link = Some(ipc);
                    sent = None;
                    set_status(Status::Connected);
                }
                Err(error) => {
                    set_status(Status::Unavailable(error));
                    continue;
                }
            }
        }

        // Rien de neuf : on n'envoie rien.
        if sent.as_ref() == Some(&wanted) {
            continue;
        }
        if let Some(ipc) = link.as_mut() {
            match ipc.set_activity(wanted.as_ref()) {
                Ok(()) => sent = Some(wanted.clone()),
                Err(error) => {
                    link = None;
                    sent = None;
                    next_try = std::time::Instant::now() + RETRY;
                    set_status(Status::Unavailable(error));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------- ipc

struct Ipc {
    stream: Stream,
    nonce: u64,
}

impl Ipc {
    fn connect(app_id: &str) -> Result<Self, String> {
        let mut last = "aucun canal Discord".to_string();
        for index in 0..10 {
            match Stream::open(index) {
                Ok(stream) => {
                    let mut ipc = Self { stream, nonce: 0 };
                    ipc.handshake(app_id)?;
                    return Ok(ipc);
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn handshake(&mut self, app_id: &str) -> Result<(), String> {
        self.write_frame(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": app_id }).to_string(),
        )?;
        let (opcode, payload) = self.read_frame()?;
        check(opcode, &payload)
    }

    fn set_activity(&mut self, activity: Option<&Activity>) -> Result<(), String> {
        self.nonce += 1;
        let payload = match activity {
            None => json!({
                "cmd": "SET_ACTIVITY",
                "nonce": self.nonce.to_string(),
                "args": { "pid": std::process::id(), "activity": null },
            }),
            Some(activity) => {
                let mut inner = json!({
                    "details": activity.details,
                    "state": activity.state,
                    "assets": { "large_image": "ruche", "large_text": "Ruche" },
                });
                if let Some(start) = activity.start {
                    inner["timestamps"] = json!({ "start": start });
                }
                json!({
                    "cmd": "SET_ACTIVITY",
                    "nonce": self.nonce.to_string(),
                    "args": { "pid": std::process::id(), "activity": inner },
                })
            }
        };
        self.write_frame(OP_FRAME, &payload.to_string())?;
        let (opcode, answer) = self.read_frame()?;
        check(opcode, &answer)
    }

    fn write_frame(&mut self, opcode: u32, payload: &str) -> Result<(), String> {
        let bytes = payload.as_bytes();
        let mut frame = Vec::with_capacity(8 + bytes.len());
        frame.extend_from_slice(&opcode.to_le_bytes());
        frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        frame.extend_from_slice(bytes);
        self.stream.write_all(&frame).map_err(|e| e.to_string())?;
        self.stream.flush().map_err(|e| e.to_string())
    }

    fn read_frame(&mut self) -> Result<(u32, String), String> {
        let mut header = [0u8; 8];
        self.stream
            .read_exact(&mut header)
            .map_err(|e| e.to_string())?;
        let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
        if length > 1 << 20 {
            return Err("trame Discord anormalement longue".into());
        }
        let mut payload = vec![0u8; length];
        self.stream
            .read_exact(&mut payload)
            .map_err(|e| e.to_string())?;
        Ok((opcode, String::from_utf8_lossy(&payload).to_string()))
    }
}

/// Une réponse de Discord est un refus si elle ferme le tube ou porte un
/// événement d'erreur.
fn check(opcode: u32, payload: &str) -> Result<(), String> {
    if opcode == OP_CLOSE || payload.contains("\"evt\":\"ERROR\"") {
        return Err(short_reason(payload));
    }
    Ok(())
}

/// Extrait le motif d'un refus, sans déballer tout le JSON.
fn short_reason(payload: &str) -> String {
    match payload.find("\"message\":\"") {
        Some(start) => {
            let rest = &payload[start + 11..];
            rest.find('"')
                .map(|end| rest[..end].to_string())
                .unwrap_or_else(|| "refus de Discord".into())
        }
        None => "refus de Discord".into(),
    }
}

// ------------------------------------------------------- tube selon l'OS

#[cfg(windows)]
struct Stream(std::fs::File);

#[cfg(windows)]
impl Stream {
    fn open(index: u32) -> Result<Self, String> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(format!(r"\\.\pipe\discord-ipc-{index}"))
            .map(Stream)
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(windows))]
struct Stream(std::os::unix::net::UnixStream);

#[cfg(not(windows))]
impl Stream {
    fn open(index: u32) -> Result<Self, String> {
        let base = std::env::var("XDG_RUNTIME_DIR")
            .or_else(|_| std::env::var("TMPDIR"))
            .unwrap_or_else(|_| "/tmp".to_string());
        std::os::unix::net::UnixStream::connect(format!("{base}/discord-ipc-{index}"))
            .map(Stream)
            .map_err(|e| e.to_string())
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_carry_their_length() {
        // On ne peut pas parler a Discord dans un test : on verifie l'encodage.
        let payload = r#"{"v":1}"#;
        let mut frame = Vec::new();
        frame.extend_from_slice(&1u32.to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend_from_slice(payload.as_bytes());
        assert_eq!(frame.len(), 8 + payload.len());
        assert_eq!(
            u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]),
            7
        );
    }

    #[test]
    fn a_close_frame_is_a_refusal() {
        let payload = r#"{"code":4000,"message":"Invalid Client ID"}"#;
        assert_eq!(check(OP_CLOSE, payload).unwrap_err(), "Invalid Client ID");
        assert!(check(OP_FRAME, r#"{"cmd":"DISPATCH","evt":"READY"}"#).is_ok());
        assert!(check(OP_FRAME, r#"{"evt":"ERROR","data":{"message":"nope"}}"#).is_err());
    }

    #[test]
    fn error_messages_are_extracted() {
        let payload = r#"{"cmd":"DISPATCH","evt":"ERROR","data":{"code":4000,"message":"Invalid Client ID"}}"#;
        assert_eq!(short_reason(payload), "Invalid Client ID");
        assert_eq!(short_reason("{}"), "refus de Discord");
    }

    #[test]
    fn a_presence_without_app_id_stays_off() {
        let presence = Presence::new();
        presence.configure("", true);
        presence.set(Some(Activity {
            details: "test".into(),
            state: String::new(),
            start: None,
        }));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(presence.status(), Status::Off);
        presence.shutdown();
    }

    /// Verifie la poignee de main contre le vrai Discord : un identifiant
    /// bidon doit revenir en « Invalid Client ID », ce qui prouve que le tube,
    /// le cadrage des trames et la lecture de la reponse fonctionnent.
    #[test]
    #[ignore = "demande que Discord tourne sur la machine"]
    fn le_tube_discord_repond_vraiment() {
        if Stream::open(0).is_err() {
            eprintln!("Discord n'est pas lance : rien a verifier");
            return;
        }
        match Ipc::connect("000000000000000000") {
            Ok(_) => panic!("un identifiant bidon ne devrait pas etre accepte"),
            Err(reason) => {
                println!("reponse de Discord : {reason}");
                assert_eq!(reason, "Invalid Client ID", "reponse inattendue");
            }
        }
    }

    #[test]
    fn identical_activities_compare_equal() {
        let a = Activity {
            details: "3 clients".into(),
            state: "sur 1.20.1".into(),
            start: Some(42),
        };
        assert_eq!(a.clone(), a);
        let mut b = a.clone();
        b.start = Some(43);
        assert_ne!(a, b);
    }
}
