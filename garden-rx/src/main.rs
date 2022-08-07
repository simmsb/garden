use std::net::SocketAddr;

use axum::body::{self, Empty, Full};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, WebSocketUpgrade};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Extension, Router};
use color_eyre::Result;
use garden_shared::{Command, DeviceStatus, PanelMessage};
use include_dir::{include_dir, Dir};
use tokio::sync::{mpsc, watch};
use tokio_stream::StreamExt;

#[cfg(not(feature = "testing_echo"))]
mod radio;

static PANEL_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../garden-panel/dist");

#[derive(Clone)]
struct State {
    status_recv: watch::Receiver<Option<DeviceStatus>>,
    cmd_in: mpsc::Sender<Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let (cmd_in, cmd_out) = tokio::sync::mpsc::channel(10);
    let (status_sender, status_recv) = watch::channel(None);

    #[cfg(not(feature = "testing_echo"))]
    std::thread::spawn(|| {
        if let Err(e) = radio::radio_side(cmd_out, status_sender) {
            println!("{:?}", e);
        }
    });

    let asset_router = Router::new()
        .route("/*path", get(static_path));

    let app = Router::new()
        .route("/ws", get(root_ws))
        .route("/", get(|| async { Redirect::to("/index.html") }))
        .fallback(asset_router)
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

async fn static_path(Path(path): Path<String>) -> impl IntoResponse {
    let path = path.trim_start_matches('/');
    let mime_type = mime_guess::from_path(path).first_or_text_plain();

    match PANEL_DIR.get_file(path) {
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(body::boxed(Empty::new()))
            .unwrap(),
        Some(file) => Response::builder()
            .status(StatusCode::OK)
            .header(
                header::CONTENT_TYPE,
                HeaderValue::from_str(mime_type.as_ref()).unwrap(),
            )
            .body(body::boxed(Full::from(file.contents())))
            .unwrap(),
    }
}

async fn root_ws(ws: WebSocketUpgrade, Extension(state): Extension<State>) -> impl IntoResponse {
    ws.on_upgrade(|s| async {
        if let Err(e) = handle_socket(s, state).await {
            eprintln!("{e:?}");
        } else {
            println!("Websocket exited");
        }
    })
}

async fn handle_socket(mut socket: WebSocket, mut state: State) -> Result<()> {
    #[cfg(feature = "testing_echo")]
    let mut status = {
        use garden_shared::StatusFlags;

        DeviceStatus {
            flags: StatusFlags::empty(),
        }
    };

    println!("Websocket connected!");

    socket
        .send(axum::extract::ws::Message::Text(
            serde_json::to_string(&PanelMessage::Hello).unwrap(),
        ))
        .await?;

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
                println!("Got status message: {:?}", v);
                if let Some(Some(v)) = v {
                    socket
                        .send(axum::extract::ws::Message::Text(
                            serde_json::to_string(&PanelMessage::Status(v)).unwrap(),
                        ))
                        .await?;
                }
            }

            cmd = socket.next() => {
                if let Some(cmd) = cmd {
                    println!("Got cmd: {:?}", cmd);
                    let cmd = cmd?;

                    match cmd {
                        Message::Text(t) => {
                            println!("got message: {}", t);
                            let cmd: Command = serde_json::from_str(&t)?;

                            #[cfg(not(feature = "testing_echo"))]
                            state.cmd_in.send(cmd).await?;

                            #[cfg(feature = "testing_echo")]
                            {
                                use garden_shared::StatusFlags;

                                match cmd {
                                    Command::PumpOn => {
                                        status.flags.set(StatusFlags::PUMP_ON, true);
                                    },
                                    Command::PumpOff => {
                                        status.flags.set(StatusFlags::PUMP_ON, false);
                                    },
                                    Command::ValveOpen => {
                                        status.flags.set(StatusFlags::VALVE_OPEN, true);
                                    },
                                    Command::ValveClose => {
                                        status.flags.set(StatusFlags::VALVE_OPEN, false);
                                    },
                                }

                                let resp = PanelMessage::Status(status);
                                println!("Sending msg: {:?}", resp);

                                socket.send(axum::extract::ws::Message::Text(
                                    serde_json::to_string(&resp).unwrap()
                                )).await?;
                            }
                        }
                        Message::Close(_) => {
                            println!("Closing websocket");
                            return Ok(());
                        },
                        _ => {}
                    }
                }
            }
        }
    }
}
