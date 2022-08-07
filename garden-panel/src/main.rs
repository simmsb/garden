#![allow(non_snake_case)]

use std::collections::VecDeque;
use std::rc::Rc;
use std::time::SystemTime;

use chrono::{DateTime, Local};
use dioxus::prelude::*;
use fermi::{use_atom_state, use_init_atom_root, use_read, use_set, Atom};
use garden_shared::{Command, DeviceStatus, StatusFlags};
use serde::{Deserialize, Serialize};
use websocket_hook::{use_ws_context, use_ws_context_provider_json, DioxusWs};

mod websocket_hook;

fn main() {
    wasm_logger::init(wasm_logger::Config::default());
    console_error_panic_hook::set_once();

    dioxus::web::launch(app);
}

#[derive(Clone)]
pub struct LogEntry {
    pub when: DateTime<Local>,
    pub msg: String,
}

impl LogEntry {
    fn new(msg: &str) -> Self {
        Self {
            when: Local::now(),
            msg: msg.to_owned(),
        }
    }
}

pub static PUMP_STATUS: Atom<Option<bool>> = |_| None;
pub static VALVE_STATUS: Atom<Option<bool>> = |_| None;
pub static COMMAND_LOG: Atom<Vec<LogEntry>> = |_| Vec::new();

fn app(cx: Scope) -> Element {
    use_init_atom_root(&cx);
    let pump_status = use_atom_state(&cx, PUMP_STATUS).clone();
    let valve_status = use_atom_state(&cx, VALVE_STATUS).clone();
    let mut log = use_atom_state(&cx, COMMAND_LOG).clone();
    use_ws_context_provider_json(&cx, env!("GARDEN_WS"), move |msg: DeviceStatus| {
        let pump_on = msg.flags.contains(StatusFlags::PUMP_ON);
        if *pump_status.current() != Some(pump_on) {
            pump_status.set(Some(pump_on));
            log.with_mut(|x| {
                x.push(LogEntry::new(if pump_on {
                    "Pump turned ON"
                } else {
                    "Pump turned OFF"
                }));
            })
        }

        let valve_on = msg.flags.contains(StatusFlags::VALVE_OPEN);
        if *valve_status.current() != Some(valve_on) {
            valve_status.set(Some(valve_on));
            log.with_mut(|x| {
                x.push(LogEntry::new(if valve_on {
                    "Valve OPENED"
                } else {
                    "Valve CLOSED"
                }));
            })
        }
    });

    cx.render(rsx!(ResponseDisplay {}))
}

fn ResponseDisplay(cx: Scope) -> Element {
    let ws = use_ws_context(&cx);
    let mut log = use_atom_state(&cx, COMMAND_LOG);
    let pump_on = (|ws: DioxusWs| {
        move |_| {
            log.with_mut(|x| {
                x.push(LogEntry::new("Enqueued pump ON msg"));
            });
            ws.send_json(&Command::PumpOn)
        }
    })(ws.clone());
    let pump_off = (|ws: DioxusWs| {
        move |_| {
            log.with_mut(|x| {
                x.push(LogEntry::new("Enqueued pump OFF msg"));
            });
            ws.send_json(&Command::PumpOff)
        }
    })(ws.clone());
    let valve_on = (|ws: DioxusWs| {
        move |_| {
            log.with_mut(|x| {
                x.push(LogEntry::new("Enqueued Valve OPEN msg"));
            });
            ws.send_json(&Command::ValveOpen)
        }
    })(ws.clone());
    let valve_off = (|ws: DioxusWs| {
        move |_| {
            log.with_mut(|x| {
                x.push(LogEntry::new("Enqueued Valve CLOSE msg"));
            });
            ws.send_json(&Command::ValveClose)
        }
    })(ws.clone());

    cx.render(rsx!(
        header {
            nav {
                class: "navbar navbar-expand-lg shadow-md py-2 bg-white relative flex items-center w-full justify-between",
                div {
                    class: "px-6 w-full flex flex-wrap items-center justify-between",
                    div {
                        class: "flex items-center",
                        span {
                            "Garden Control Panel"
                        }
                    }
                }
            }
        }
        main {
            div {
                class: "justify-center flex space-x-2 bg-gray-50 text-gray-800 py-6 px-6",
                button {
                    class: "inline-block px-6 py-2.5 bg-green-500 text-white font-medium text-xs leading-tight uppercase rounded shadow-md hover:bg-green-600 hover:shadow-lg focus:bg-green-600 focus:shadow-lg focus:outline-none focus:ring-0 active:bg-green-700 active:shadow-lg transition duration-150 ease-in-out",
                    onclick: pump_on,
                    "Enable Pump"
                }
                button {
                    class: "inline-block px-6 py-2.5 bg-red-600 text-white font-medium text-xs leading-tight uppercase rounded shadow-md hover:bg-red-700 hover:shadow-lg focus:bg-red-700 focus:shadow-lg focus:outline-none focus:ring-0 active:bg-red-800 active:shadow-lg transition duration-150 ease-in-out",
                    onclick: pump_off,
                    "Disable Pump"
                }
                PumpStatus {}
            }
            div {
                class: "justify-center flex space-x-2 bg-gray-50 text-gray-800 py-6 px-6",
                button {
                    class: "inline-block px-6 py-2.5 bg-green-500 text-white font-medium text-xs leading-tight uppercase rounded shadow-md hover:bg-green-600 hover:shadow-lg focus:bg-green-600 focus:shadow-lg focus:outline-none focus:ring-0 active:bg-green-700 active:shadow-lg transition duration-150 ease-in-out",
                    onclick: valve_on,
                    "Enable Valve"
                }
                button {
                    class: "inline-block px-6 py-2.5 bg-red-600 text-white font-medium text-xs leading-tight uppercase rounded shadow-md hover:bg-red-700 hover:shadow-lg focus:bg-red-700 focus:shadow-lg focus:outline-none focus:ring-0 active:bg-red-800 active:shadow-lg transition duration-150 ease-in-out",
                    onclick: valve_off,
                    "Disable Valve"
                }
                ValveStatus {}
            }
            CommandLog {}
        }
    ))
}

fn CommandLog(cx: Scope) -> Element {
    let log = use_read(&cx, COMMAND_LOG);

    cx.render(rsx!(
        div {
            class: "mx-auto drop-shadow-lg m-4 rounded-lg font-mono w-8/12",
            log.iter().rev().map(|l| {
                let msg = &l.msg;
                let ts = l.when.format("%Y-%m-%d %H:%M:%S");
                let k = l.when.timestamp_nanos();
                rsx!(
                    div {
                        class: "mt-4 flex",
                        key: "{k}",
                        span {
                            class: "text-green-400",
                            "[{ts}]"
                        }
                        p {
                            class: "flex-1 typing items-center pl-2",
                            "{msg}",
                            br {}
                        }
                    }
                )
            })
        }
    ))
}

fn PumpStatus(cx: Scope) -> Element {
    let pump_status = match use_read(&cx, PUMP_STATUS) {
        Some(true) => "On ✅",
        Some(false) => "Off ❌",
        None => "Unknown",
    };
    cx.render(rsx!(span { "Pump Status: {pump_status}" }))
}

fn ValveStatus(cx: Scope) -> Element {
    let valve_status = match use_read(&cx, VALVE_STATUS) {
        Some(true) => "On ✅",
        Some(false) => "Off ❌",
        None => "Unknown",
    };
    cx.render(rsx!(span { "Valve Status: {valve_status}" }))
}
