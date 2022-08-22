#![no_std]

use core::time::Duration;

#[allow(unused_imports)]
use micromath::F32Ext;

use uom::si::{
    f32::{ElectricalResistance, Pressure, Ratio, ThermodynamicTemperature},
    pressure::pascal,
    ratio::percent,
    thermodynamic_temperature::degree_celsius,
};

#[derive(serde::Serialize, serde::Deserialize, Debug, PartialEq, Eq)]
pub struct DevAddr(pub u16);

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct MoistureReading {
    pub clocks: u16,
    pub duration: Duration,
}

impl MoistureReading {
    pub fn per_second(&self) -> f32 {
        self.clocks as f32 / self.duration.as_secs_f32()
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct MoistureSensorReport {
    pub moisture: heapless::Vec<MoistureReading, 8>,
}

impl MoistureSensorReport {
    pub fn sanity_check(self, last: Option<&Self>) -> Option<Self> {
        if let Some(last) = last {
            if self.moisture.len() != last.moisture.len() {
                return None;
            }

            for (a, b) in self.moisture.iter().zip(&last.moisture) {
                if (a.per_second() - b.per_second()).abs() > 5.0 {
                    return None;
                }
            }
        }

        Some(self)
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BME688SensorReport {
    pub temp: ThermodynamicTemperature,
    pub pressure: Pressure,
    pub humidity: Ratio,
    pub gas_resistance: ElectricalResistance,
}

impl BME688SensorReport {
    pub fn sanity_check(self, last: Option<&Self>) -> Option<BME688SensorReport> {
        if self.temp > ThermodynamicTemperature::new::<degree_celsius>(80.0) {
            return None;
        }

        if let Some(last) = last {
            if (last.temp.get::<degree_celsius>() - self.temp.get::<degree_celsius>()).abs() > 20.0
            {
                return None;
            }
            if (last.pressure - self.pressure).abs() > Pressure::new::<pascal>(100.0) {
                return None;
            }

            if (last.humidity - self.humidity).abs() > Ratio::new::<percent>(10.0) {
                return None;
            }
        }

        Some(self)
    }
}

bitflags::bitflags! {
    #[derive(serde::Serialize, serde::Deserialize)]
    pub struct StatusFlags: u8 {
        const PUMP_ON    = 0b01;
        const VALVE_OPEN = 0b10;
    }
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub struct DeviceStatus {
    pub flags: StatusFlags,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum Message {
    MoistureReport(MoistureSensorReport),
    BME688Report(BME688SensorReport),
    StatusUpdate(DeviceStatus),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub enum Command {
    SyncFlags(StatusFlags),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub enum UiCommand {
    PumpOn,
    PumpOff,
    ValveOpen,
    ValveClose,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub struct Transmission<T> {
    pub src: DevAddr,
    pub msg: T,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub enum PanelMessage {
    Hello,
    Status(DeviceStatus),
    DesiredStatus(StatusFlags),
}
