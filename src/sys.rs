//! Ce que le launcher demande au systeme : memoire, etat des process, fenetres,
//! affinite CPU et chiffrement local des jetons.
//!
//! Windows est la cible principale ; les autres plateformes ont une version
//! degradee mais compilable (pas de DPAPI, pas de detection de fenetre).

/// Nombre de coeurs logiques, 4 par defaut si le systeme ne repond pas.
pub fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Masque d'affinite pour la n-ieme instance, ou `None` si on ne restreint rien.
pub fn affinity_mask(index: usize, cores: usize) -> Option<usize> {
    let total = cpu_count();
    if cores == 0 || cores >= total {
        return None;
    }
    let start = (index * cores) % total;
    let mut mask = 0usize;
    for i in 0..cores {
        mask |= 1 << ((start + i) % total);
    }
    Some(mask)
}

pub fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, HWND, LPARAM, LocalFree, SYSTEMTIME,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CryptProtectData, CryptUnprotectData,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::SystemInformation::{
        GetLocalTime, GlobalMemoryStatusEx, MEMORYSTATUSEX,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
        SetProcessAffinityMask,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// (RAM physique totale, RAM disponible) en mebioctets.
    pub fn memory_mb() -> (u64, u64) {
        let mut st: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        st.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut st) } == 0 {
            return (0, 0);
        }
        (st.ullTotalPhys / 1_048_576, st.ullAvailPhys / 1_048_576)
    }

    /// Date et heure locales : (annee, mois, jour, heure, minute, seconde).
    pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        (
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond,
        )
    }

    /// Working set du process, en mebioctets (0 si le process est parti).
    pub fn process_rss_mb(pid: u32) -> u64 {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return 0;
            }
            let mut counters: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            counters.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            let ok = GetProcessMemoryInfo(handle, &mut counters, counters.cb);
            CloseHandle(handle);
            if ok == 0 {
                0
            } else {
                (counters.WorkingSetSize as u64) / 1_048_576
            }
        }
    }

    struct Search {
        pid: u32,
        found: bool,
    }

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> i32 {
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut owner: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
        if owner == search.pid
            && unsafe { IsWindowVisible(hwnd) } != 0
            && unsafe { GetWindowTextLengthW(hwnd) } > 0
        {
            search.found = true;
            return 0; // on arrete l'enumeration
        }
        1
    }

    /// Vrai des que le process a une fenetre visible : le client est affiche.
    pub fn has_visible_window(pid: u32) -> bool {
        let mut search = Search { pid, found: false };
        unsafe { EnumWindows(Some(enum_cb), &mut search as *mut Search as LPARAM) };
        search.found
    }

    pub fn set_affinity(pid: u32, mask: usize) {
        unsafe {
            let handle: HANDLE = OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            );
            if handle.is_null() {
                return;
            }
            SetProcessAffinityMask(handle, mask);
            CloseHandle(handle);
        }
    }

    /// Chiffre pour la session Windows courante (DPAPI). Rend une chaine hex.
    pub fn protect(text: &str) -> Option<String> {
        unsafe {
            let mut input = text.as_bytes().to_vec();
            let blob_in = CRYPT_INTEGER_BLOB {
                cbData: input.len() as u32,
                pbData: input.as_mut_ptr(),
            };
            let mut blob_out: CRYPT_INTEGER_BLOB = std::mem::zeroed();
            let label: Vec<u16> = "Ruche\0".encode_utf16().collect();
            let ok = CryptProtectData(
                &blob_in,
                label.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut blob_out,
            );
            if ok == 0 {
                return None;
            }
            let slice =
                std::slice::from_raw_parts(blob_out.pbData, blob_out.cbData as usize).to_vec();
            LocalFree(blob_out.pbData as *mut c_void);
            Some(super::hex_encode(&slice))
        }
    }

    pub fn unprotect(hex: &str) -> Option<String> {
        let mut raw = super::hex_decode(hex)?;
        unsafe {
            let blob_in = CRYPT_INTEGER_BLOB {
                cbData: raw.len() as u32,
                pbData: raw.as_mut_ptr(),
            };
            let mut blob_out: CRYPT_INTEGER_BLOB = std::mem::zeroed();
            let ok = CryptUnprotectData(
                &blob_in,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut blob_out,
            );
            if ok == 0 {
                return None;
            }
            let slice =
                std::slice::from_raw_parts(blob_out.pbData, blob_out.cbData as usize).to_vec();
            LocalFree(blob_out.pbData as *mut c_void);
            String::from_utf8(slice).ok()
        }
    }
}

#[cfg(not(windows))]
mod imp {
    /// Sans API systeme, on reste en temps UTC.
    pub fn local_time() -> (u16, u16, u16, u16, u16, u16) {
        let secs = super::super::config::now_secs();
        let day = secs / 86_400;
        let rest = secs % 86_400;
        (
            1970,
            1,
            (day % 31 + 1) as u16,
            (rest / 3600) as u16,
            ((rest / 60) % 60) as u16,
            (rest % 60) as u16,
        )
    }

    pub fn memory_mb() -> (u64, u64) {
        let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let field = |key: &str| -> u64 {
            text.lines()
                .find(|l| l.starts_with(key))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
                / 1024
        };
        (field("MemTotal:"), field("MemAvailable:"))
    }

    pub fn process_rss_mb(pid: u32) -> u64 {
        std::fs::read_to_string(format!("/proc/{pid}/statm"))
            .ok()
            .and_then(|s| {
                s.split_whitespace()
                    .nth(1)
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .map(|pages| pages * 4 / 1024)
            .unwrap_or(0)
    }

    /// Pas d'equivalent portable : la file retombe sur la taille du process.
    pub fn has_visible_window(_pid: u32) -> bool {
        false
    }

    pub fn set_affinity(_pid: u32, _mask: usize) {}

    /// Sans DPAPI, le jeton est stocke tel quel (voir le README).
    pub fn protect(text: &str) -> Option<String> {
        Some(text.to_string())
    }

    pub fn unprotect(text: &str) -> Option<String> {
        Some(text.to_string())
    }
}

pub use imp::{has_visible_window, local_time, memory_mb, process_rss_mb, set_affinity};

/// `HH:MM:SS` local, pour le journal.
pub fn clock() -> String {
    let (_y, _mo, _d, h, m, s) = local_time();
    format!("{h:02}:{m:02}:{s:02}")
}

/// `AAAAMMJJ-HHMMSS` local, pour les noms de fichiers de log.
pub fn file_stamp() -> String {
    let (y, mo, d, h, m, s) = local_time();
    format!("{y:04}{mo:02}{d:02}-{h:02}{m:02}{s:02}")
}

const DPAPI_PREFIX: &str = "dpapi:";

/// Protege un secret avant ecriture sur disque ; renvoie le texte tel quel si
/// le systeme ne sait pas chiffrer.
pub fn protect_secret(text: &str) -> String {
    if text.is_empty() || text.starts_with(DPAPI_PREFIX) {
        return text.to_string();
    }
    match imp::protect(text) {
        Some(hex) => format!("{DPAPI_PREFIX}{hex}"),
        None => text.to_string(),
    }
}

/// Inverse de [`protect_secret`] ; chaine vide si le blob n'est pas dechiffrable
/// (typiquement : fichier recopie depuis une autre session Windows).
pub fn reveal_secret(text: &str) -> String {
    match text.strip_prefix(DPAPI_PREFIX) {
        None => text.to_string(),
        Some(hex) => imp::unprotect(hex).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip() {
        let bytes = [0u8, 1, 42, 255, 128];
        assert_eq!(hex_decode(&hex_encode(&bytes)).unwrap(), bytes);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }

    #[test]
    fn secret_roundtrip() {
        let secret = "M.C123_BAY.0.u.-CkFakeRefreshToken";
        let sealed = protect_secret(secret);
        assert_eq!(reveal_secret(&sealed), secret);
        // deja protege : on ne rechiffre pas
        assert_eq!(protect_secret(&sealed), sealed);
        // texte en clair : rendu tel quel
        assert_eq!(reveal_secret("brut"), "brut");
    }

    #[test]
    fn affinity_groups_are_disjoint() {
        let cores = 2;
        let a = affinity_mask(0, cores).unwrap();
        let b = affinity_mask(1, cores).unwrap();
        assert_eq!(a & b, 0, "deux instances voisines partagent des coeurs");
        assert_eq!(a.count_ones(), cores as u32);
    }

    #[test]
    fn memory_is_readable() {
        let (total, avail) = memory_mb();
        assert!(total > 0, "RAM totale illisible");
        assert!(avail <= total);
    }
}
