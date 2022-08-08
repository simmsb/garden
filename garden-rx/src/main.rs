use std::net::SocketAddr;

use axum::body::{self, Empty, Full};
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{Path, WebSocketUpgrade};
use axum::http::{header, HeaderValue, Response, StatusCode};
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{Extension, Router};
use color_eyre::Result;
use garden_shared::{DeviceStatus, PanelMessage, StatusFlags, UiCommand};
use include_dir::{include_dir, Dir};
use tokio::sync::watch;
use tokio_stream::StreamExt;

use crate::radio::DESIRED_STATE;

mod radio;

static PANEL_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../garden-panel/dist");

#[derive(Clone)]
struct State {
    status_recv: watch::Receiver<Option<DeviceStatus>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let (status_sender, status_recv) = watch::channel(None);

    std::thread::spawn(|| {
        if let Err(e) = radio::radio_side(status_sender) {
            println!("{:?}", e);
        }
    });

    let asset_router = Router::new().route("/*path", get(static_path));

    let app = Router::new()
        .route("/ws", get(root_ws))
        .route("/", get(|| async { Redirect::to("/index.html") }))
        .fallback(asset_router)
        .layer(Extension(State { status_recv }))
        .layer(tower_http::compression::CompressionLayer::new())
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300")));

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
    println!("Websocket connected!");

    socket
        .send(Message::Text(
            serde_json::to_string(&PanelMessage::Hello).unwrap(),
        ))
        .await?;

    let v = state.status_recv.borrow_and_update().clone();
    if let Some(v) = v {
        let c = PanelMessage::Status(v);
        socket
            .send(Message::Text(serde_json::to_string(&c).unwrap()))
            .await?;
    }

    let c = PanelMessage::DesiredStatus(*DESIRED_STATE.lock().unwrap());
    socket
        .send(Message::Text(serde_json::to_string(&c).unwrap()))
        .await?;

    let mut status_stream = tokio_stream::wrappers::WatchStream::new(state.status_recv);

    loop {
        tokio::select! {
            v = status_stream.next() => {
                println!("Got status message: {:?}", v);
                if let Some(Some(v)) = v {
                    socket
                        .send(Message::Text(
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
                            let cmd: UiCommand = serde_json::from_str(&t)?;

                            let c = {
                                let mut desired_state = DESIRED_STATE.lock().unwrap();

                                match cmd {
                                    UiCommand::PumpOn => {
                                        desired_state.set(StatusFlags::PUMP_ON, true);
                                    },
                                    UiCommand::PumpOff => {
                                        desired_state.set(StatusFlags::PUMP_ON, false);
                                    },
                                    UiCommand::ValveOpen => {
                                        desired_state.set(StatusFlags::VALVE_OPEN, true);
                                    },
                                    UiCommand::ValveClose => {
                                        desired_state.set(StatusFlags::VALVE_OPEN, false);
                                    },
                                }

                                PanelMessage::DesiredStatus(*desired_state)
                            };
                            println!("Sending message {:?}", c);
                            socket
                                .send(Message::Text(serde_json::to_string(&c).unwrap()))
                                .await?;
                        }
                        Message::Ping(msg) => {
                            socket.send(Message::Pong(msg)).await?;
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
