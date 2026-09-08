use super::*;
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, TimeZone, Timelike, Utc};

impl Vm {
    pub(super) fn datetime_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        match selector {
            0x160 => {
                self.write_time(arg(0), Utc::now())?;
                Ok(0)
            }
            0x161 => {
                let factor = arg(0);
                if factor == 0 {
                    return Err(VmError::InvalidTime);
                }
                Ok(Utc::now().timestamp().div_euclid(factor as i64) as u32)
            }
            0x168..=0x16b => {
                let simple = selector >= 0x16a;
                let time = if simple {
                    DateTime::from_timestamp(arg(0) as i32 as i64 * arg(1) as i64, 0)
                        .ok_or(VmError::InvalidTime)?
                } else {
                    let hi = self.read_glk_word(arg(0), 0)?;
                    let lo = self.read_glk_word(arg(0), 1)?;
                    let micros = self.read_glk_word(arg(0), 2)? as i32 as i64;
                    let seconds = ((hi as i64) << 32) | lo as i64;
                    DateTime::from_timestamp(
                        seconds
                            .checked_add(micros.div_euclid(1_000_000))
                            .ok_or(VmError::InvalidTime)?,
                        micros.rem_euclid(1_000_000) as u32 * 1000,
                    )
                    .ok_or(VmError::InvalidTime)?
                };
                let destination = arg(if simple { 2 } else { 1 });
                let date = if selector & 1 == 0 {
                    date_fields(time)
                } else {
                    date_fields(time.with_timezone(&Local))
                };
                for (i, value) in date.into_iter().enumerate() {
                    self.write_glk_word(destination, i, value)?;
                }
                Ok(0)
            }
            0x16c..=0x16f => {
                let mut date = [0i32; 8];
                for (i, value) in date.iter_mut().enumerate() {
                    *value = self.read_glk_word(arg(0), i)? as i32;
                }
                let month = date[1] as i64 - 1;
                let year = (date[0] as i64)
                    .checked_add(month.div_euclid(12))
                    .and_then(|v| i32::try_from(v).ok())
                    .ok_or(VmError::InvalidTime)?;
                let day = NaiveDate::from_ymd_opt(year, month.rem_euclid(12) as u32 + 1, 1)
                    .ok_or(VmError::InvalidTime)?;
                let naive = day
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .checked_add_signed(Duration::days(date[2] as i64 - 1))
                    .and_then(|v| v.checked_add_signed(Duration::hours(date[4] as i64)))
                    .and_then(|v| v.checked_add_signed(Duration::minutes(date[5] as i64)))
                    .and_then(|v| v.checked_add_signed(Duration::seconds(date[6] as i64)))
                    .and_then(|v| v.checked_add_signed(Duration::microseconds(date[7] as i64)))
                    .ok_or(VmError::InvalidTime)?;
                let time = if selector & 1 == 0 {
                    Utc.from_utc_datetime(&naive)
                } else {
                    Local
                        .from_local_datetime(&naive)
                        .earliest()
                        .ok_or(VmError::InvalidTime)?
                        .with_timezone(&Utc)
                };
                if selector >= 0x16e {
                    if arg(1) == 0 {
                        return Err(VmError::InvalidTime);
                    }
                    Ok(time.timestamp().div_euclid(arg(1) as i64) as u32)
                } else {
                    self.write_time(arg(1), time)?;
                    Ok(0)
                }
            }
            _ => Ok(0),
        }
    }
    fn read_glk_word(&mut self, address: u32, index: usize) -> Result<u32, VmError> {
        if address == u32::MAX {
            self.stack.pop_u32()
        } else {
            self.memory.read32(
                address
                    .checked_add(index as u32 * 4)
                    .ok_or(VmError::InvalidTime)?,
            )
        }
    }
    fn write_glk_word(&mut self, address: u32, index: usize, value: u32) -> Result<(), VmError> {
        self.write_glk_reference(
            if matches!(address, 0 | u32::MAX) {
                address
            } else {
                address
                    .checked_add(index as u32 * 4)
                    .ok_or(VmError::InvalidTime)?
            },
            value,
        )
    }
    fn write_time(&mut self, address: u32, time: DateTime<Utc>) -> Result<(), VmError> {
        let seconds = time.timestamp();
        for (i, value) in [
            (seconds >> 32) as u32,
            seconds as u32,
            time.timestamp_subsec_micros(),
        ]
        .into_iter()
        .enumerate()
        {
            self.write_glk_word(address, i, value)?;
        }
        Ok(())
    }
}
fn date_fields<T: TimeZone>(time: DateTime<T>) -> [u32; 8] {
    [
        time.year() as u32,
        time.month(),
        time.day(),
        time.weekday().num_days_from_sunday(),
        time.hour(),
        time.minute(),
        time.second(),
        time.timestamp_subsec_micros(),
    ]
}
