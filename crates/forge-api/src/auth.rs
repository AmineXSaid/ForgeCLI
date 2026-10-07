//! Which endpoint a request goes to and where its URL and key came from, so
//! an error can name the one variable to fix. Keys are per provider: a
//! Messages key is never sent to an OpenAI-compatible endpoint, nor the reverse.

/// The wire protocol of the endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Messages,
    OpenAi,
}

/// What supplied the key a request carries. Each is a variable or setting name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyFrom {
    /// No key at all.
    #[default]
    None,
    /// `FORGE_API_KEY` (Messages, `x-api-key`).
    ApiKey,
    /// `FORGE_AUTH_TOKEN` (Messages, `Authorization: Bearer`).
    AuthToken,
    /// Both of the above.
    ApiKeyAndToken,
    /// `FORGE_OPENAI_API_KEY` (OpenAI-compatible, `Authorization: Bearer`).
    OpenAiKey,
    /// A key helper command: `apiKeyHelper` or `openai.apiKeyHelper`.
    Helper(&'static str),
}

/// The endpoint a request went to and where its settings came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Origin {
    pub backend: Backend,
    /// The variable or setting that chose the URL (`FORGE_OPENAI_BASE_URL`,
    /// `openai.baseUrl`, `FORGE_BASE_URL`, `baseUrl`), or `None` for the default host.
    pub url_from: Option<&'static str>,
    pub key_from: KeyFrom,
}

impl Origin {
    /// The name to check when the URL is wrong.
    pub fn url_var(&self) -> &'static str {
        self.url_from.unwrap_or(match self.backend {
            Backend::Messages => "FORGE_BASE_URL",
            Backend::OpenAi => "FORGE_OPENAI_BASE_URL",
        })
    }

    /// `system/init.apiKeySource`: the variable or setting the key came from, or "none".
    pub fn api_key_source(&self) -> &'static str {
        match self.key_from {
            KeyFrom::None => "none",
            KeyFrom::ApiKey | KeyFrom::ApiKeyAndToken => "FORGE_API_KEY",
            KeyFrom::AuthToken => "FORGE_AUTH_TOKEN",
            KeyFrom::OpenAiKey => "FORGE_OPENAI_API_KEY",
            KeyFrom::Helper(s) => s,
        }
    }

    /// For a 401 or 403: which key was refused, and what to change.
    pub fn auth_hint(&self) -> String {
        let what = match (self.backend, self.key_from) {
            (Backend::OpenAi, KeyFrom::None) => "No key was sent: set FORGE_OPENAI_API_KEY to this endpoint's key. \
                 FORGE_API_KEY and FORGE_AUTH_TOKEN are never sent to an OpenAI-compatible endpoint."
                .to_string(),
            (Backend::OpenAi, KeyFrom::Helper(s)) | (Backend::Messages, KeyFrom::Helper(s)) => format!(
                "The endpoint did not accept the key printed by {s}: run that command yourself and check what it prints."
            ),
            (Backend::OpenAi, _) => {
                "The endpoint did not accept the key in FORGE_OPENAI_API_KEY: check its value.".to_string()
            }
            (Backend::Messages, KeyFrom::AuthToken) => "The endpoint did not accept the token in FORGE_AUTH_TOKEN \
                 (sent as Authorization: Bearer). If it expects an x-api-key header, set FORGE_API_KEY instead."
                .to_string(),
            (Backend::Messages, KeyFrom::ApiKeyAndToken) => "The endpoint did not accept FORGE_API_KEY (sent as \
                 x-api-key) or FORGE_AUTH_TOKEN (sent as Authorization: Bearer): keep the one it expects and unset \
                 the other."
                .to_string(),
            (Backend::Messages, KeyFrom::None) => {
                "No key was sent: set FORGE_API_KEY (or FORGE_AUTH_TOKEN for a gateway).".to_string()
            }
            (Backend::Messages, _) => "The endpoint did not accept the key in FORGE_API_KEY (sent as x-api-key). If \
                 it is a gateway that expects a bearer token, set FORGE_AUTH_TOKEN instead."
                .to_string(),
        };
        format!("{what} `forge doctor` shows what is configured.")
    }

    /// For a 404: the model name, or the URL.
    pub fn not_found_hint(&self) -> String {
        match self.backend {
            Backend::OpenAi => format!(
                "Check the model name (--model or FORGE_MODEL) and {}: Forge adds /chat/completions to it, so it \
                 usually ends in /v1.",
                self.url_var()
            ),
            Backend::Messages => format!("Check the model name (--model or FORGE_MODEL) and {}.", self.url_var()),
        }
    }

    /// For a failed connection.
    pub fn network_hint(&self) -> String {
        format!("Check {}, your network connection and proxy settings.", self.url_var())
    }
}

/// A URL as it may be shown: no user name, password, query or fragment.
pub fn display_url(u: &str) -> String {
    match reqwest::Url::parse(u) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string().trim_end_matches('/').to_string()
        }
        Err(_) => u.split(['?', '#']).next().unwrap_or("").to_string(),
    }
}

/// Only `http://` and `https://` URLs with a host.
pub fn parse_base_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|e| e.to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return Err("not an http:// or https:// URL".into());
    }
    Ok(url)
}

/// A host on this machine or a private network, where servers often need no key.
pub fn is_local_url(u: &reqwest::Url) -> bool {
    let Some(host) = u.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local(),
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(backend: Backend, key_from: KeyFrom) -> Origin {
        Origin { backend, url_from: None, key_from }
    }

    #[test]
    fn openai_401_hint_names_forge_openai_api_key() {
        let h = o(Backend::OpenAi, KeyFrom::None).auth_hint();
        assert!(h.contains("set FORGE_OPENAI_API_KEY") && h.contains("never sent"), "{h}");
        assert!(!h.starts_with("Check FORGE_API_KEY"));
        let h = o(Backend::OpenAi, KeyFrom::OpenAiKey).auth_hint();
        assert!(h.contains("did not accept the key in FORGE_OPENAI_API_KEY"), "{h}");
        assert!(h.ends_with("`forge doctor` shows what is configured."));
    }

    #[test]
    fn messages_401_hint_suggests_the_other_header() {
        assert!(o(Backend::Messages, KeyFrom::ApiKey).auth_hint().contains("set FORGE_AUTH_TOKEN instead"));
        assert!(o(Backend::Messages, KeyFrom::AuthToken).auth_hint().contains("set FORGE_API_KEY instead"));
        assert!(o(Backend::Messages, KeyFrom::ApiKeyAndToken).auth_hint().contains("unset the other"));
        assert!(o(Backend::Messages, KeyFrom::Helper("apiKeyHelper")).auth_hint().contains("printed by apiKeyHelper"));
    }

    #[test]
    fn network_and_404_hints_name_the_active_url_variable() {
        let openai = o(Backend::OpenAi, KeyFrom::OpenAiKey);
        assert!(openai.network_hint().contains("FORGE_OPENAI_BASE_URL"));
        assert!(openai.not_found_hint().contains("FORGE_OPENAI_BASE_URL") && openai.not_found_hint().contains("/v1"));
        let set = Origin { url_from: Some("openai.baseUrl"), ..openai };
        assert!(set.network_hint().contains("openai.baseUrl") && !set.network_hint().contains("FORGE_BASE_URL"));
        assert!(o(Backend::Messages, KeyFrom::ApiKey).network_hint().contains("FORGE_BASE_URL"));
    }

    #[test]
    fn local_urls_need_no_key() {
        for u in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8000/v1",
            "http://[::1]:8080/v1",
            "http://192.168.1.20:8000/v1",
            "http://10.0.0.5/v1",
            "http://ollama.localhost/v1",
            "http://[fd00::1]/v1",
        ] {
            assert!(is_local_url(&parse_base_url(u).unwrap()), "{u}");
        }
        for u in ["https://gw.example.com/v1", "http://8.8.8.8/v1"] {
            assert!(!is_local_url(&parse_base_url(u).unwrap()), "{u}");
        }
        assert!(parse_base_url("gw.example.com/v1").is_err());
        assert!(parse_base_url("ftp://gw.example.com").is_err());
    }

    #[test]
    fn display_url_hides_userinfo_and_query() {
        assert_eq!(display_url("https://u:p@gw.example.com/v1/models?key=x#f"), "https://gw.example.com/v1/models");
    }

    #[test]
    fn api_key_source_labels() {
        assert_eq!(o(Backend::OpenAi, KeyFrom::None).api_key_source(), "none");
        assert_eq!(o(Backend::OpenAi, KeyFrom::OpenAiKey).api_key_source(), "FORGE_OPENAI_API_KEY");
        assert_eq!(o(Backend::Messages, KeyFrom::AuthToken).api_key_source(), "FORGE_AUTH_TOKEN");
        assert_eq!(o(Backend::Messages, KeyFrom::Helper("apiKeyHelper")).api_key_source(), "apiKeyHelper");
    }
}
