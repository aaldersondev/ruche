//! Connexion Microsoft par « device code ».
//!
//! Enchainement : Microsoft -> Xbox Live -> XSTS -> Minecraft Services -> profil.
//! Aucun mot de passe ne passe par le launcher : l'utilisateur valide un code
//! sur `microsoft.com/link`, et seuls des jetons reviennent ici.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{Account, AccountKind, now_secs, sanitize};

const TENANT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const SCOPE: &str = "XboxLive.signin offline_access";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";
const MC_STORE: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const USER_AGENT: &str = concat!("ruche/", env!("CARGO_PKG_VERSION"));

/// Echec d'authentification, avec un message deja presentable.
#[derive(Debug, Clone)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthError {}

fn err<T>(message: impl Into<String>) -> Result<T, AuthError> {
    Err(AuthError(message.into()))
}

/// Code a saisir sur la page Microsoft.
#[derive(Clone, Debug)]
pub struct DeviceFlow {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

fn agent() -> ureq::Agent {
    // On veut lire le corps des reponses 4xx (codes XErr, messages AADSTS).
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// POST formulaire vers Microsoft ; renvoie le json quel que soit le statut.
fn post_form(url: &str, fields: &[(&str, &str)]) -> Result<Value, AuthError> {
    let mut response = agent()
        .post(url)
        .send_form(fields.iter().copied())
        .map_err(|e| AuthError(format!("réseau injoignable : {e}")))?;
    read_json(&mut response)
}

/// POST json vers Xbox Live / Minecraft Services.
fn post_json(url: &str, body: &Value) -> Result<(u16, Value), AuthError> {
    let mut response = agent()
        .post(url)
        .header("Accept", "application/json")
        .send_json(body)
        .map_err(|e| AuthError(format!("réseau injoignable : {e}")))?;
    let status = response.status().as_u16();
    Ok((status, read_json(&mut response)?))
}

fn get_json(url: &str, token: &str) -> Result<(u16, Value), AuthError> {
    let mut response = agent()
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/json")
        .call()
        .map_err(|e| AuthError(format!("réseau injoignable : {e}")))?;
    let status = response.status().as_u16();
    Ok((status, read_json(&mut response)?))
}

fn read_json(response: &mut ureq::http::Response<ureq::Body>) -> Result<Value, AuthError> {
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| AuthError(format!("réponse illisible : {e}")))?;
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text)
        .map_err(|_| AuthError(format!("réponse inattendue : {}", truncate(&text, 160))))
}

fn truncate(text: &str, max: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        cleaned.chars().take(max).collect::<String>() + "..."
    }
}

fn ms_error(value: &Value) -> String {
    let raw = value
        .get("error_description")
        .and_then(Value::as_str)
        .or_else(|| value.get("error").and_then(Value::as_str))
        .unwrap_or("réponse Microsoft incomprise");
    truncate(raw.split(['\r', '\n']).next().unwrap_or(raw), 200)
}

/// Demande un code d'appairage a Microsoft.
pub fn device_start(client_id: &str) -> Result<DeviceFlow, AuthError> {
    if client_id.trim().is_empty() {
        return err("aucun identifiant d'application Azure n'est configuré");
    }
    let value = post_form(
        &format!("{TENANT}/devicecode"),
        &[("client_id", client_id), ("scope", SCOPE)],
    )?;
    let Some(device_code) = value.get("device_code").and_then(Value::as_str) else {
        return err(ms_error(&value));
    };
    Ok(DeviceFlow {
        device_code: device_code.to_string(),
        user_code: value
            .get("user_code")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        verification_uri: value
            .get("verification_uri")
            .and_then(Value::as_str)
            .unwrap_or("https://www.microsoft.com/link")
            .to_string(),
        interval: value.get("interval").and_then(Value::as_u64).unwrap_or(5),
        expires_in: value
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(900),
    })
}

/// Jetons Microsoft renvoyes par le flux device code.
struct MsTokens {
    access_token: String,
    refresh_token: String,
}

/// Interroge Microsoft jusqu'a validation du code (ou abandon).
fn device_wait(
    client_id: &str,
    flow: &DeviceFlow,
    should_stop: &dyn Fn() -> bool,
    on_wait: &dyn Fn(u64),
) -> Result<MsTokens, AuthError> {
    let mut interval = flow.interval.max(1);
    let deadline = now_secs() + flow.expires_in;
    while now_secs() < deadline {
        for _ in 0..interval {
            if should_stop() {
                return err("connexion annulée");
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        on_wait(deadline.saturating_sub(now_secs()));
        let value = post_form(
            &format!("{TENANT}/token"),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", client_id),
                ("device_code", &flow.device_code),
            ],
        )?;
        match value.get("error").and_then(Value::as_str) {
            None => return tokens_from(&value),
            Some("authorization_pending") => continue,
            Some("slow_down") => interval += 5,
            Some("expired_token") => return err("le code a expiré, recommence"),
            Some("authorization_declined") => {
                return err("connexion refusée sur la page Microsoft");
            }
            Some(_) => return err(ms_error(&value)),
        }
    }
    err("délai dépassé : le code n'a pas été validé")
}

fn tokens_from(value: &Value) -> Result<MsTokens, AuthError> {
    let Some(access) = value.get("access_token").and_then(Value::as_str) else {
        return err(ms_error(value));
    };
    Ok(MsTokens {
        access_token: access.to_string(),
        refresh_token: value
            .get("refresh_token")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn refresh_tokens(client_id: &str, refresh_token: &str) -> Result<MsTokens, AuthError> {
    let value = post_form(
        &format!("{TENANT}/token"),
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("scope", SCOPE),
            ("refresh_token", refresh_token),
        ],
    )?;
    if value.get("access_token").is_none() {
        return err(format!(
            "session Microsoft expirée ({}) — reconnecte le compte",
            ms_error(&value)
        ));
    }
    tokens_from(&value)
}

fn xbox_authenticate(ms_access_token: &str) -> Result<String, AuthError> {
    let (_status, value) = post_json(
        XBL_URL,
        &json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access_token}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }),
    )?;
    match value.get("Token").and_then(Value::as_str) {
        Some(token) => Ok(token.to_string()),
        None => err("Xbox Live a refusé la session Microsoft"),
    }
}

/// Message clair pour les refus les plus frequents de XSTS.
fn xerr_message(code: &str) -> String {
    match code {
        "2148916233" => "ce compte Microsoft n'a pas de profil Xbox : crée-le sur xbox.com".into(),
        "2148916235" => "Xbox Live n'est pas disponible dans le pays du compte".into(),
        "2148916236" | "2148916237" => "le compte doit passer une vérification adulte".into(),
        "2148916238" => "compte enfant : il doit être rattaché à un groupe familial".into(),
        other => format!("XSTS a refusé le compte (code {other})"),
    }
}

struct XstsToken {
    token: String,
    user_hash: String,
    xuid: String,
}

fn xsts_authorize(xbl_token: &str) -> Result<XstsToken, AuthError> {
    let (status, value) = post_json(
        XSTS_URL,
        &json!({
            "Properties": {"SandboxId": "RETAIL", "UserTokens": [xbl_token]},
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT",
        }),
    )?;
    if status == 401 {
        let code = value
            .get("XErr")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into());
        return Err(AuthError(xerr_message(code.trim_matches('"'))));
    }
    let Some(token) = value.get("Token").and_then(Value::as_str) else {
        return err("XSTS n'a pas renvoyé de jeton");
    };
    let claims = value
        .get("DisplayClaims")
        .and_then(|c| c.get("xui"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(Value::Null);
    Ok(XstsToken {
        token: token.to_string(),
        user_hash: claims
            .get("uhs")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        xuid: claims
            .get("xid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

fn minecraft_login(xsts: &XstsToken) -> Result<(String, u64), AuthError> {
    let (status, value) = post_json(
        MC_LOGIN,
        &json!({
            "identityToken": format!("XBL3.0 x={};{}", xsts.user_hash, xsts.token),
        }),
    )?;
    match value.get("access_token").and_then(Value::as_str) {
        Some(token) => Ok((
            token.to_string(),
            value
                .get("expires_in")
                .and_then(Value::as_u64)
                .unwrap_or(86_400),
        )),
        None => err(format!(
            "Minecraft Services a refusé la connexion (HTTP {status})"
        )),
    }
}

/// Pseudo et UUID reels du compte.
fn minecraft_profile(mc_token: &str) -> Result<(String, String), AuthError> {
    let (status, value) = get_json(MC_PROFILE, mc_token)?;
    if status == 404 {
        let (_s, store) = get_json(MC_STORE, mc_token)?;
        let owns = store
            .get("items")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        return if owns {
            err("aucun profil : choisis d'abord un pseudo sur minecraft.net")
        } else {
            err("ce compte ne possède pas Minecraft Java Edition")
        };
    }
    let (Some(id), Some(name)) = (
        value.get("id").and_then(Value::as_str),
        value.get("name").and_then(Value::as_str),
    ) else {
        return err(format!("profil Minecraft illisible (HTTP {status})"));
    };
    Ok((name.to_string(), super::dashed(id)))
}

/// Derniere partie de la chaine, commune a la connexion et au rafraichissement.
fn finish(client_id: &str, tokens: MsTokens) -> Result<Account, AuthError> {
    let xbl = xbox_authenticate(&tokens.access_token)?;
    let xsts = xsts_authorize(&xbl)?;
    let (mc_token, expires_in) = minecraft_login(&xsts)?;
    let (name, uuid) = minecraft_profile(&mc_token)?;
    Ok(Account {
        instance: sanitize(&name),
        name,
        uuid,
        kind: AccountKind::Microsoft,
        access_token: mc_token,
        // 5 minutes de marge : on ne veut pas lancer avec un jeton qui meurt.
        expires_at: now_secs() + expires_in.saturating_sub(300),
        refresh_token: crate::sys::protect_secret(&tokens.refresh_token),
        xuid: xsts.xuid,
        client_id: client_id.to_string(),
        selected: true,
        ..Default::default()
    })
}

/// Connexion complete. `on_code` recoit le code des qu'il est disponible.
pub fn login_device(
    client_id: &str,
    on_code: impl Fn(&DeviceFlow),
    should_stop: impl Fn() -> bool,
    on_wait: impl Fn(u64),
) -> Result<Account, AuthError> {
    let flow = device_start(client_id)?;
    on_code(&flow);
    let tokens = device_wait(client_id, &flow, &should_stop, &on_wait)?;
    finish(client_id, tokens)
}

/// Renouvelle la session Minecraft si elle a expire. Modifie le compte en place.
pub fn ensure_valid(
    account: &mut Account,
    fallback_client_id: &str,
    log: impl Fn(String),
) -> Result<bool, AuthError> {
    if account.kind != AccountKind::Microsoft {
        return Ok(false);
    }
    if !account.access_token.is_empty() && account.expires_at > now_secs() {
        return Ok(false);
    }
    let client_id = if account.client_id.is_empty() {
        fallback_client_id.to_string()
    } else {
        account.client_id.clone()
    };
    let refresh = crate::sys::reveal_secret(&account.refresh_token);
    if refresh.is_empty() {
        return err(format!(
            "aucun jeton de rafraîchissement : reconnecte {}",
            account.name
        ));
    }
    log(format!(
        "[{}] session Microsoft expirée, renouvellement",
        account.name
    ));
    let tokens = refresh_tokens(&client_id, &refresh)?;
    let fresh = finish(&client_id, tokens)?;

    // On garde ce qui appartient a la configuration locale.
    account.name = fresh.name;
    account.uuid = fresh.uuid;
    account.access_token = fresh.access_token;
    account.expires_at = fresh.expires_at;
    account.refresh_token = fresh.refresh_token;
    account.xuid = fresh.xuid;
    account.client_id = fresh.client_id;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_client_id_is_refused_before_any_call() {
        let e = device_start("   ").unwrap_err();
        assert!(e.0.contains("Azure"), "message inattendu : {}", e.0);
    }

    #[test]
    fn xerr_codes_are_explained() {
        assert!(xerr_message("2148916233").contains("xbox.com"));
        assert!(xerr_message("2148916238").contains("familial"));
        assert!(xerr_message("42").contains("42"));
    }

    #[test]
    fn microsoft_errors_are_summarised() {
        let value = json!({
            "error": "invalid_grant",
            "error_description": "AADSTS7000012: The grant was obtained\r\nfor another tenant."
        });
        let text = ms_error(&value);
        assert!(text.starts_with("AADSTS7000012"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn offline_accounts_never_hit_the_network() {
        let mut account = Account::offline("Alt1");
        assert!(!ensure_valid(&mut account, "cid", |_| {}).unwrap());
    }

    #[test]
    fn premium_without_refresh_token_asks_for_a_reconnection() {
        let mut account = Account {
            name: "Prem".into(),
            kind: AccountKind::Microsoft,
            expires_at: 0,
            ..Default::default()
        };
        let e = ensure_valid(&mut account, "cid", |_| {}).unwrap_err();
        assert!(e.0.contains("reconnecte Prem"), "{}", e.0);
    }

    #[test]
    fn a_valid_session_is_left_alone() {
        let mut account = Account {
            name: "Prem".into(),
            kind: AccountKind::Microsoft,
            access_token: "encore-bon".into(),
            expires_at: now_secs() + 3600,
            ..Default::default()
        };
        assert!(!ensure_valid(&mut account, "cid", |_| {}).unwrap());
        assert_eq!(account.access_token, "encore-bon");
    }
}
