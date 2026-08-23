//! Connexion Microsoft par « device code ».
//!
//! Enchaînement : Microsoft → Xbox Live → XSTS → Minecraft Services → profil.
//! Aucun mot de passe ne passe par le launcher : l'utilisateur valide un code
//! sur `microsoft.com/link`, et seuls des jetons reviennent ici.

use std::time::Duration;

use serde_json::{Value, json};

use crate::config::{Account, AccountKind, now_secs, sanitize};
use crate::i18n::{Lang, Strings, fill};

const TENANT: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0";
const SCOPE: &str = "XboxLive.signin offline_access";
const XBL_URL: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_URL: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_LOGIN: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";
const MC_STORE: &str = "https://api.minecraftservices.com/entitlements/mcstore";
const USER_AGENT: &str = concat!("ruche/", env!("CARGO_PKG_VERSION"));

/// Échec d'authentification, avec un message déjà présentable.
#[derive(Debug, Clone)]
pub struct AuthError(pub String);

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthError {}

type Res<T> = Result<T, AuthError>;

fn err<T>(message: impl Into<String>) -> Res<T> {
    Err(AuthError(message.into()))
}

/// Code à saisir sur la page Microsoft.
#[derive(Clone, Debug)]
pub struct DeviceFlow {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
    pub expires_in: u64,
}

/// Jetons Microsoft renvoyés par le flux device code.
struct MsTokens {
    access_token: String,
    refresh_token: String,
}

struct XstsToken {
    token: String,
    user_hash: String,
    xuid: String,
}

/// Client HTTP porteur de la langue : tous les messages d'erreur en sortent.
struct Client {
    s: &'static Strings,
    agent: ureq::Agent,
}

impl Client {
    fn new(lang: Lang) -> Self {
        // On veut lire le corps des réponses 4xx (codes XErr, messages AADSTS).
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            s: lang.strings(),
            agent,
        }
    }

    // ------------------------------------------------------------- transport

    fn post_form(&self, url: &str, fields: &[(&str, &str)]) -> Res<Value> {
        let mut response = self
            .agent
            .post(url)
            .send_form(fields.iter().copied())
            .map_err(|e| self.net(e))?;
        self.read_json(&mut response)
    }

    fn post_json(&self, url: &str, body: &Value) -> Res<(u16, Value)> {
        let mut response = self
            .agent
            .post(url)
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|e| self.net(e))?;
        let status = response.status().as_u16();
        Ok((status, self.read_json(&mut response)?))
    }

    fn get_json(&self, url: &str, token: &str) -> Res<(u16, Value)> {
        let mut response = self
            .agent
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .call()
            .map_err(|e| self.net(e))?;
        let status = response.status().as_u16();
        Ok((status, self.read_json(&mut response)?))
    }

    fn net(&self, error: ureq::Error) -> AuthError {
        AuthError(fill(self.s.auth_network, &[&error.to_string()]))
    }

    fn read_json(&self, response: &mut ureq::http::Response<ureq::Body>) -> Res<Value> {
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|e| AuthError(fill(self.s.auth_unreadable, &[&e.to_string()])))?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .map_err(|_| AuthError(fill(self.s.auth_unexpected, &[&truncate(&text, 160)])))
    }

    /// Message d'erreur renvoyé par Microsoft, ramené à une ligne.
    fn ms_error(&self, value: &Value) -> String {
        let raw = value
            .get("error_description")
            .and_then(Value::as_str)
            .or_else(|| value.get("error").and_then(Value::as_str))
            .unwrap_or(self.s.auth_ms_unknown);
        truncate(raw.split(['\r', '\n']).next().unwrap_or(raw), 200)
    }

    // ---------------------------------------------------------- device code

    fn device_start(&self, client_id: &str) -> Res<DeviceFlow> {
        if client_id.trim().is_empty() {
            return err(self.s.auth_no_client_id);
        }
        let value = self.post_form(
            &format!("{TENANT}/devicecode"),
            &[("client_id", client_id), ("scope", SCOPE)],
        )?;
        let Some(device_code) = value.get("device_code").and_then(Value::as_str) else {
            return err(self.ms_error(&value));
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

    /// Interroge Microsoft jusqu'à validation du code (ou abandon).
    fn device_wait(
        &self,
        client_id: &str,
        flow: &DeviceFlow,
        should_stop: &dyn Fn() -> bool,
        on_wait: &dyn Fn(u64),
    ) -> Res<MsTokens> {
        let mut interval = flow.interval.max(1);
        let deadline = now_secs() + flow.expires_in;
        while now_secs() < deadline {
            for _ in 0..interval {
                if should_stop() {
                    return err(self.s.auth_cancelled);
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            on_wait(deadline.saturating_sub(now_secs()));
            let value = self.post_form(
                &format!("{TENANT}/token"),
                &[
                    ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                    ("client_id", client_id),
                    ("device_code", &flow.device_code),
                ],
            )?;
            match value.get("error").and_then(Value::as_str) {
                None => return self.tokens_from(&value),
                Some("authorization_pending") => continue,
                Some("slow_down") => interval += 5,
                Some("expired_token") => return err(self.s.auth_code_expired),
                Some("authorization_declined") => return err(self.s.auth_declined),
                Some(_) => return err(self.ms_error(&value)),
            }
        }
        err(self.s.auth_timeout)
    }

    fn tokens_from(&self, value: &Value) -> Res<MsTokens> {
        let Some(access) = value.get("access_token").and_then(Value::as_str) else {
            return err(self.ms_error(value));
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

    fn refresh_tokens(&self, client_id: &str, refresh_token: &str) -> Res<MsTokens> {
        let value = self.post_form(
            &format!("{TENANT}/token"),
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("scope", SCOPE),
                ("refresh_token", refresh_token),
            ],
        )?;
        if value.get("access_token").is_none() {
            return err(fill(self.s.auth_session_expired, &[&self.ms_error(&value)]));
        }
        self.tokens_from(&value)
    }

    // ------------------------------------------------- xbox live / minecraft

    fn xbox_authenticate(&self, ms_access_token: &str) -> Res<String> {
        let (_status, value) = self.post_json(
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
            None => err(self.s.auth_xbox_refused),
        }
    }

    /// Message clair pour les refus les plus fréquents de XSTS.
    fn xerr_message(&self, code: &str) -> String {
        match code {
            "2148916233" => self.s.auth_no_xbox_profile.to_string(),
            "2148916235" => self.s.auth_country.to_string(),
            "2148916236" | "2148916237" => self.s.auth_adult.to_string(),
            "2148916238" => self.s.auth_child.to_string(),
            other => fill(self.s.auth_xsts, &[other]),
        }
    }

    fn xsts_authorize(&self, xbl_token: &str) -> Res<XstsToken> {
        let (status, value) = self.post_json(
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
            return Err(AuthError(self.xerr_message(code.trim_matches('"'))));
        }
        let Some(token) = value.get("Token").and_then(Value::as_str) else {
            return err(self.s.auth_xsts_no_token);
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

    fn minecraft_login(&self, xsts: &XstsToken) -> Res<(String, u64)> {
        let (status, value) = self.post_json(
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
            None => err(fill(self.s.auth_mc_refused, &[&status.to_string()])),
        }
    }

    /// Pseudo et UUID réels du compte.
    fn minecraft_profile(&self, mc_token: &str) -> Res<(String, String)> {
        let (status, value) = self.get_json(MC_PROFILE, mc_token)?;
        if status == 404 {
            let (_s, store) = self.get_json(MC_STORE, mc_token)?;
            let owns = store
                .get("items")
                .and_then(Value::as_array)
                .map(|items| !items.is_empty())
                .unwrap_or(false);
            return if owns {
                err(self.s.auth_no_profile)
            } else {
                err(self.s.auth_not_owned)
            };
        }
        let (Some(id), Some(name)) = (
            value.get("id").and_then(Value::as_str),
            value.get("name").and_then(Value::as_str),
        ) else {
            return err(fill(self.s.auth_profile_unreadable, &[&status.to_string()]));
        };
        Ok((name.to_string(), super::dashed(id)))
    }

    /// Dernière partie de la chaîne, commune à la connexion et au renouvellement.
    fn finish(&self, client_id: &str, tokens: MsTokens) -> Res<Account> {
        let xbl = self.xbox_authenticate(&tokens.access_token)?;
        let xsts = self.xsts_authorize(&xbl)?;
        let (mc_token, expires_in) = self.minecraft_login(&xsts)?;
        let (name, uuid) = self.minecraft_profile(&mc_token)?;
        Ok(Account {
            instance: sanitize(&name),
            name,
            uuid,
            kind: AccountKind::Microsoft,
            access_token: mc_token,
            // 5 minutes de marge : on ne lance pas avec un jeton qui va mourir.
            expires_at: now_secs() + expires_in.saturating_sub(300),
            refresh_token: crate::sys::protect_secret(&tokens.refresh_token),
            xuid: xsts.xuid,
            client_id: client_id.to_string(),
            selected: true,
            ..Default::default()
        })
    }
}

fn truncate(text: &str, max: usize) -> String {
    let cleaned = text.replace(['\r', '\n'], " ");
    if cleaned.chars().count() <= max {
        cleaned
    } else {
        cleaned.chars().take(max).collect::<String>() + "…"
    }
}

/// Demande un code d'appairage à Microsoft.
pub fn device_start(client_id: &str, lang: Lang) -> Res<DeviceFlow> {
    Client::new(lang).device_start(client_id)
}

/// Connexion complète. `on_code` reçoit le code dès qu'il est disponible.
pub fn login_device(
    client_id: &str,
    lang: Lang,
    on_code: impl Fn(&DeviceFlow),
    should_stop: impl Fn() -> bool,
    on_wait: impl Fn(u64),
) -> Res<Account> {
    let client = Client::new(lang);
    let flow = client.device_start(client_id)?;
    on_code(&flow);
    let tokens = client.device_wait(client_id, &flow, &should_stop, &on_wait)?;
    client.finish(client_id, tokens)
}

/// Renouvelle la session Minecraft si elle a expiré. Modifie le compte en place.
/// Renvoie `true` quand quelque chose a changé et mérite d'être sauvegardé.
pub fn ensure_valid(
    account: &mut Account,
    fallback_client_id: &str,
    lang: Lang,
    log: impl Fn(String),
) -> Res<bool> {
    if account.kind != AccountKind::Microsoft {
        return Ok(false);
    }
    if !account.access_token.is_empty() && account.expires_at > now_secs() {
        return Ok(false);
    }
    let client = Client::new(lang);
    let client_id = if account.client_id.is_empty() {
        fallback_client_id.to_string()
    } else {
        account.client_id.clone()
    };
    let refresh = crate::sys::reveal_secret(&account.refresh_token);
    if refresh.is_empty() {
        return err(fill(client.s.auth_no_refresh, &[&account.name]));
    }
    log(fill(client.s.auth_refreshing, &[&account.name]));
    let tokens = client.refresh_tokens(&client_id, &refresh)?;
    let fresh = client.finish(&client_id, tokens)?;

    // On garde ce qui appartient à la configuration locale.
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
        let e = device_start("   ", Lang::Fr).unwrap_err();
        assert!(e.0.contains("Azure"), "message inattendu : {}", e.0);
        let e = device_start("   ", Lang::En).unwrap_err();
        assert!(e.0.contains("Azure"), "message inattendu : {}", e.0);
    }

    #[test]
    fn xerr_codes_are_explained_in_both_languages() {
        let fr = Client::new(Lang::Fr);
        assert!(fr.xerr_message("2148916233").contains("xbox.com"));
        assert!(fr.xerr_message("2148916238").contains("familial"));
        assert!(fr.xerr_message("42").contains("42"));

        let en = Client::new(Lang::En);
        assert!(en.xerr_message("2148916238").contains("family"));
        assert_ne!(en.xerr_message("2148916233"), fr.xerr_message("2148916233"));
    }

    #[test]
    fn microsoft_errors_are_summarised() {
        let client = Client::new(Lang::Fr);
        let value = json!({
            "error": "invalid_grant",
            "error_description": "AADSTS7000012: The grant was obtained\r\nfor another tenant."
        });
        let text = client.ms_error(&value);
        assert!(text.starts_with("AADSTS7000012"));
        assert!(!text.contains('\n'));
    }

    #[test]
    fn offline_accounts_never_hit_the_network() {
        let mut account = Account::offline("Alt1");
        assert!(!ensure_valid(&mut account, "cid", Lang::Fr, |_| {}).unwrap());
    }

    #[test]
    fn premium_without_refresh_token_asks_for_a_reconnection() {
        let mut account = Account {
            name: "Prem".into(),
            kind: AccountKind::Microsoft,
            expires_at: 0,
            ..Default::default()
        };
        let e = ensure_valid(&mut account, "cid", Lang::Fr, |_| {}).unwrap_err();
        assert!(e.0.contains("Prem"), "{}", e.0);
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
        assert!(!ensure_valid(&mut account, "cid", Lang::Fr, |_| {}).unwrap());
        assert_eq!(account.access_token, "encore-bon");
    }
}
