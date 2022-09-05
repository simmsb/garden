#[cfg_attr(not(feature = "std"), no_std)]
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

#[derive(displaydoc::Display, Debug)]
#[cfg_attr(feature = "std", derive(thiserror::Error))]
pub enum MoistureSensorValidationError {
    /// The lengths of two moisture readings are different
    DifferingLengths,

    /// The difference between two readings of sensor {sensor} is too large: {diff}
    LargeDelta { sensor: usize, diff: f32 },
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct MoistureSensorReport {
    pub moisture: heapless::Vec<MoistureReading, 8>,
}

impl MoistureSensorReport {
    pub fn sanity_check(self, last: Option<&Self>) -> Result<Self, MoistureSensorValidationError> {
        if let Some(last) = last {
            if self.moisture.len() != last.moisture.len() {
                return Err(MoistureSensorValidationError::DifferingLengths);
            }

            for (n, (a, b)) in self.moisture.iter().zip(&last.moisture).enumerate() {
                let absolute_diff = (a.per_second() - b.per_second()).abs();
                if absolute_diff > 15.0 {
                    return Err(MoistureSensorValidationError::LargeDelta {
                        sensor: n,
                        diff: absolute_diff,
                    });
                }
            }
        }

        Ok(self)
    }
}

#[derive(displaydoc::Display, Debug)]
#[cfg_attr(feature = "std", derive(thiserror::Error))]
pub enum BME688SensorValidationError {
    /// The reported temperature of {0} is incredibly unlikely and probably sensor erorr
    UnreasonablyHot(f32),

    /// The delta of {0} between temperature readings is larger than should be possible, discarding as an error
    LargeTempDelta(f32),

    /// The delta of {0} between pressure readings is larger than should be possible, discarding as an error
    LargePressureDelta(f32),

    /// The delta of {0} between humidity readings is larger than should be possible, discarding as an error
    LargeHumidityDelta(f32),
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct BME688SensorReport {
    pub temp: ThermodynamicTemperature,
    pub pressure: Pressure,
    pub humidity: Ratio,
    pub gas_resistance: ElectricalResistance,
}

impl BME688SensorReport {
    pub fn sanity_check(
        self,
        last: Option<&Self>,
    ) -> Result<BME688SensorReport, BME688SensorValidationError> {
        if self.temp > ThermodynamicTemperature::new::<degree_celsius>(80.0) {
            return Err(BME688SensorValidationError::UnreasonablyHot(
                self.temp.get::<degree_celsius>(),
            ));
        }

        if let Some(last) = last {
            let abs = (last.temp.get::<degree_celsius>() - self.temp.get::<degree_celsius>()).abs();
            if abs > 20.0 {
                return Err(BME688SensorValidationError::LargeTempDelta(abs));
            }
            let abs = (last.pressure - self.pressure).abs().get::<pascal>();
            if abs > 100.0 {
                return Err(BME688SensorValidationError::LargePressureDelta(abs));
            }

            let abs = (last.humidity - self.humidity).abs().get::<percent>();
            if abs > 50.0 {
                return Err(BME688SensorValidationError::LargeHumidityDelta(abs));
            }
        }

        Ok(self)
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
    Reset,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy)]
pub enum UiCommand {
    PumpOn,
    PumpOff,
    ValveOpen,
    ValveClose,
    Reset,
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
