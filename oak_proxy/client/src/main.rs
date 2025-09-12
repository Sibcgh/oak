//
// Copyright 2025 The Project Oak Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//

use std::{net::SocketAddr, sync::Arc};

use clap::Parser;
use oak_proxy_lib::{
    config::{self, ClientConfig},
    proxy::{proxy, PeerRole},
    websocket::{read_message, write_message},
};
use oak_session::{ClientSession, ProtocolEngine, Session};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the TOML configuration file.
    #[arg(long, value_parser = crate::config::load_toml::<ClientConfig>)]
    config: ClientConfig,

    /// The SocketAddr where the proxy should listen (e.g., "127.0.0.1:9090").
    /// This will override the value in the config file.
    #[arg(long, env = "OAK_PROXY_CLIENT_LISTEN_ADDRESS")]
    listen_address: Option<SocketAddr>,

    /// The WebSocket URL of the server proxy.
    /// This will override the value in the config file.
    #[arg(long, env = "OAK_PROXY_SERVER_URL")]
    server_proxy_url: Option<Url>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    let Args { mut config, listen_address, server_proxy_url } = Args::parse();

    // The command-line arguments override the values from the config file.
    if let Some(listen_address) = listen_address {
        config.listen_address = listen_address;
    }
    if let Some(server_proxy_url) = server_proxy_url {
        config.server_proxy_url = server_proxy_url;
    }

    let listener = TcpListener::bind(config.listen_address).await?;
    log::info!("[Client] Listening on {}", config.listen_address);

    let config = Arc::new(config);
    loop {
        let (stream, peer_address) = listener.accept().await?;
        log::info!("[Client] Accepted connection from {}", peer_address);
        let config = config.clone();
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream, &config).await {
                log::error!("[Client] Error handling connection: {:?}", err);
            }
        });
    }
}

async fn handle_connection(app_stream: TcpStream, config: &ClientConfig) -> anyhow::Result<()> {
    let (mut server_proxy_stream, _) =
        tokio_tungstenite::connect_async(&config.server_proxy_url).await?;
    log::info!("[Client] Connected to server proxy at {}", config.server_proxy_url);

    let client_config = config::build_session_config(
        &config.attestation_generators,
        &config.attestation_verifiers,
    )?;
    let mut session = ClientSession::create(client_config)?;

    // Handshake
    while !session.is_open() {
        if let Some(request) = session.get_outgoing_message()? {
            write_message(&mut server_proxy_stream, &request).await?;
        }

        if !session.is_open() {
            let response = read_message(&mut server_proxy_stream).await?;
            session.put_incoming_message(response)?;
        }
    }

    log::info!("[Client] Oak Session established with server proxy.");

    proxy::<
        ClientSession,
        oak_proto_rust::oak::session::v1::SessionResponse,
        oak_proto_rust::oak::session::v1::SessionRequest,
    >(PeerRole::Client, session, app_stream, server_proxy_stream, config.keep_alive_interval)
    .await
}
