//! Identite des comptes : UUID hors-ligne et session Microsoft.

pub mod msa;

/// UUID d'un compte hors-ligne, calcule comme le fait le serveur :
/// UUID v3 (MD5) de `OfflinePlayer:<pseudo>`.
pub fn offline_uuid(name: &str) -> String {
    let mut bytes = md5::compute(format!("OfflinePlayer:{name}").as_bytes()).0;
    bytes[6] = (bytes[6] & 0x0f) | 0x30; // version 3
    bytes[8] = (bytes[8] & 0x3f) | 0x80; // variante RFC 4122
    format_uuid(&bytes)
}

/// Met des tirets sur un UUID de 32 caracteres hexadecimaux.
pub fn dashed(plain: &str) -> String {
    if plain.len() != 32 || plain.contains('-') {
        return plain.to_string();
    }
    format!(
        "{}-{}-{}-{}-{}",
        &plain[0..8],
        &plain[8..12],
        &plain[12..16],
        &plain[16..20],
        &plain[20..32]
    )
}

fn format_uuid(bytes: &[u8; 16]) -> String {
    let hex = crate::sys::hex_encode(bytes);
    dashed(&hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_uuid_matches_the_server() {
        // Valeurs de reference : ce que calcule un serveur en online-mode=false.
        assert_eq!(
            offline_uuid("Notch"),
            "b50ad385-829d-3141-a216-7e7d7539ba7f"
        );
        assert_eq!(offline_uuid("jeb_"), "a762f560-4fce-3236-812a-b80efff0b62b");
    }

    #[test]
    fn uuid_is_version_3() {
        let uuid = offline_uuid("Alt1");
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "3", "mauvaise version d'UUID");
        let variant = uuid.chars().nth(19).unwrap();
        assert!(
            ['8', '9', 'a', 'b'].contains(&variant),
            "mauvaise variante : {variant}"
        );
    }

    #[test]
    fn dashes_are_added_once() {
        assert_eq!(
            dashed("8203642fea0f46c1938615d671b5df90"),
            "8203642f-ea0f-46c1-9386-15d671b5df90"
        );
        let already = "8203642f-ea0f-46c1-9386-15d671b5df90";
        assert_eq!(dashed(already), already);
    }
}
