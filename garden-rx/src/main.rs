use std::net::SocketAddr;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use color_eyre::Result;
use garden_shared::{Command, DeviceStatus};
use tokio::sync::{mpsc, watch};
use tokio_stream::StreamExt;

mod radio;

#[derive(Clone)]
struct State {
    status_recv: watch::Receiver<Option<DeviceStatus>>,
    cmd_in: mpsc::Sender<Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let (cmd_in, cmd_out) = tokio::sync::mpsc::channel(10);
    let (status_sender, status_recv) = watch::channel(None);

    std::thread::spawn(|| {
        if let Err(e) = radio::radio_side(cmd_out, status_sender) {
            println!("{:?}", e);
        }
    });

    let app = Router::new()
        .route("/ws", get(root_ws))
        .layer(Extension(State {
            cmd_in,
            status_recv,
        }));

    let addr = SocketAddr::from(([0, 0, 0, 0], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn root_ws(ws: WebSocketUpgrade, Extension(state): Extension<State>) -> impl IntoResponse {
    ws.on_upgrade(|s| async {
        if let Err(e) = handle_socket(s, state).await {
            eprintln!("{e:?}");
        }
    })
}

async fn handle_socket(mut socket: WebSocket, mut state: State) -> Result<()> {
    let v = state.status_recv.borrow_and_update().clone();
    if let Some(v) = v {
        socket
            .send(axum::extract::ws::Message::Text(
                serde_json::to_string(&v).unwrap(),
            ))
            .await?;
    }

    let mut status_stream = tokio_stream::wrappers::WatchStream::new(state.status_recv);

    loop {
        tokio::select! {
            v = status_stream.next() => {
                if let Some(Some(v)) = v {
                    socket
                        .send(axum::extract::ws::Message::Text(
                            serde_json::to_string(&v).unwrap(),
                        ))
                        .await?;
                }
            }

            cmd = socket.next() => {
                if let Some(cmd) = cmd {
                    let cmd = cmd?;

                    match cmd {
                        Message::Text(t) => {
                            let cmd: Command = serde_json::from_str(&t)?;
                            state.cmd_in.send(cmd).await?;
                        }
                        Message::Close(_) => {
                            return Ok(());
                        },
                        _ => {}
                    }
                }
            }
        }
    }
}
