//! The socket layer: accept, read one request, reply, close.
//!
//! Deliberately thin. Everything that decides anything lives in [`crate::protocol`] or in
//! `skysaga_state`, both of which are pure and tested without a network.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use skysaga_state::{AppState, LoginError};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

use crate::protocol::{LoginReply, LoginRequest, LoginResult, ProtocolError, HEADER_SIZE};

/// The port the client's launcher expects, unchanged from the C#.
pub const DEFAULT_PORT: u16 = 10106;

#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub addr: SocketAddr,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
        }
    }
}

impl AuthConfig {
    /// Read the listen address from the environment (`SKYSAGA_AUTH_PORT`).
    pub fn from_env() -> Self {
        let port = std::env::var("SKYSAGA_AUTH_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(DEFAULT_PORT);

        Self {
            addr: SocketAddr::from(([0, 0, 0, 0], port)),
        }
    }
}

/// Accept connections until the listener fails, handling each in its own task.
///
/// The C# handles one connection at a time in a single loop, so a client that connects and
/// then stalls blocks every other player from signing in.
pub async fn serve(listener: TcpListener, state: Arc<AppState>) -> io::Result<()> {
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                warn!(%error, "accept failed");
                continue;
            }
        };

        let state = Arc::clone(&state);

        tokio::spawn(async move {
            if let Err(error) = handle_connection(stream, state).await {
                debug!(%peer, %error, "login connection closed");
            }
        });
    }
}

/// One request, one reply, connection closed.
async fn handle_connection(mut stream: TcpStream, state: Arc<AppState>) -> io::Result<()> {
    let request = match read_request(&mut stream).await? {
        Ok(request) => request,
        Err(error) => {
            debug!(%error, "discarding malformed login packet");

            return Ok(());
        }
    };

    let reply = authenticate(&request, &state);

    info!(
        username = %request.username,
        result = ?reply.result,
        "login",
    );

    stream.write_all(&reply.to_bytes()).await?;
    stream.flush().await
}

/// Read exactly one `LoginRequest`.
///
/// `read_exact` on the header and then on the body, because TCP may split the packet
/// anywhere. The C# does a single `Read()` and assumes it got all 123 bytes.
async fn read_request(
    stream: &mut TcpStream,
) -> io::Result<Result<LoginRequest, ProtocolError>> {
    let mut header_bytes = [0u8; HEADER_SIZE];

    stream.read_exact(&mut header_bytes).await?;

    let header = match crate::protocol::Header::parse(&header_bytes) {
        Ok(header) => header,
        Err(error) => return Ok(Err(error)),
    };

    if header.id != crate::protocol::packet_id::LOGIN_REQUEST {
        return Ok(Err(ProtocolError::UnexpectedId(header.id)));
    }

    // Cap the allocation: a hostile peer must not be able to ask for a 64 KiB buffer and a
    // read that never completes. The only packet accepted here is a fixed 123 bytes.
    if usize::from(header.length) != LoginRequest::SIZE {
        return Ok(Err(ProtocolError::LengthMismatch {
            declared: usize::from(header.length),
            actual: LoginRequest::SIZE,
        }));
    }

    let mut body = vec![0u8; header.body_len()];

    stream.read_exact(&mut body).await?;

    Ok(LoginRequest::parse_body(&body))
}

/// Check the credentials and build the reply.
fn authenticate(request: &LoginRequest, state: &AppState) -> LoginReply {
    match state.authenticate(&request.username, &request.password) {
        Ok(session) => LoginReply::accepted(&request.username, session.token),

        Err(LoginError::BadCredentials) => {
            LoginReply::rejected(&request.username, LoginResult::WrongPassword)
        }

        Err(LoginError::NoSuchAccount) => {
            LoginReply::rejected(&request.username, LoginResult::NoSuchAccount)
        }
    }
}
