#![no_std]

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct DevAddr(pub u16);

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct MoistureReading {
    pub clocks: u16,
    pub duration_ms: u16,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct SensorReport {
    pub moisture: heapless::Vec<MoistureReading, 8>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum Message {
    Report(SensorReport),
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Transmission<T> {
    pub src: DevAddr,
    pub msg: T,
}
