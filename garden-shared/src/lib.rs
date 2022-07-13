#![no_std]

use core::time::Duration;

use uom::si::f32::{ElectricalResistance, Pressure, Ratio, ThermodynamicTemperature};

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct DevAddr(pub u16);

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct MoistureReading {
    pub clocks: u16,
    pub duration: Duration,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct MoistureSensorReport {
    pub moisture: heapless::Vec<MoistureReading, 8>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BME688SensorReport {
    pub temp: ThermodynamicTemperature,
    pub pressure: Pressure,
    pub humidity: Ratio,
    pub gas_resistance: ElectricalResistance,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum Message {
    MoistureReport(MoistureSensorReport),
    BME688Report(BME688SensorReport),
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Transmission<T> {
    pub src: DevAddr,
    pub msg: T,
}
