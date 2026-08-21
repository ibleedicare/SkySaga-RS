//! Web server configuration, read from the environment.

use std::net::SocketAddr;

/// Ports unchanged from the C#, so the existing launch scripts keep working.
pub const DEFAULT_HTTP_PORT: u16 = 5164;

/// The 2017 builds (Alpha V10 b36731) request the API over https. A single port cannot serve
/// both schemes, so both are served and either client can be online at once.
pub const DEFAULT_HTTPS_PORT: u16 = 5165;

/// The UDP port the RakNet game server listens on.
pub const DEFAULT_GAME_PORT: u16 = 42069;

#[derive(Debug, Clone)]
pub struct WebConfig {
    pub http_port: u16,
    pub https_port: u16,

    /// Address handed to the client for the web and game servers. Must be reachable from
    /// wherever the client runs — a client in a VM or a container sees no `127.0.0.1` of
    /// ours.
    pub public_ip: String,

    pub game_port: u16,

    /// Datacentre name reported by `/api/game-conductor/geonode`.
    pub datacentre: String,

    /// Shared secret for the admin API, from `SKYSAGA_ADMIN_TOKEN`.
    ///
    /// `None` means the admin routes are not mounted at all. A server started normally has no
    /// admin surface rather than one behind a default token.
    pub admin_token: Option<String>,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            http_port: DEFAULT_HTTP_PORT,
            https_port: DEFAULT_HTTPS_PORT,
            public_ip: "127.0.0.1".to_owned(),
            game_port: DEFAULT_GAME_PORT,
            datacentre: "UK".to_owned(),
            admin_token: None,
        }
    }
}

impl WebConfig {
    /// Read the configuration from the environment, keeping the C#'s variable names.
    pub fn from_env() -> Self {
        let default = Self::default();

        fn port(name: &str, fallback: u16) -> u16 {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(fallback)
        }

        Self {
            http_port: port("SKYSAGA_WEB_PORT", default.http_port),
            https_port: port("SKYSAGA_WEB_HTTPS_PORT", default.https_port),
            admin_token: std::env::var("SKYSAGA_ADMIN_TOKEN").ok().filter(|t| !t.is_empty()),
            public_ip: std::env::var("SKYSAGA_PUBLIC_IP").unwrap_or(default.public_ip),
            game_port: port("SKYSAGA_GAME_PORT", default.game_port),
            datacentre: std::env::var("SKYSAGA_DATACENTRE").unwrap_or(default.datacentre),
        }
    }

    pub fn http_addr(&self) -> SocketAddr {
        SocketAddr::from(([0, 0, 0, 0], self.http_port))
    }
}
