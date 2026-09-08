use super::*;
use chrono::{
    DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Utc,
};

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
                let time = normalize_date(date).and_then(|naive| {
                    if selector & 1 == 0 {
                        Some(Utc.from_utc_datetime(&naive))
                    } else {
                        local_time(&Local, naive).map(|v| v.with_timezone(&Utc))
                    }
                });
                if selector >= 0x16e {
                    if arg(1) == 0 {
                        return Err(VmError::InvalidTime);
                    }
                    Ok(time.map_or(u32::MAX, |v| v.timestamp().div_euclid(arg(1) as i64) as u32))
                } else {
                    if let Some(time) = time {
                        self.write_time(arg(1), time)?;
                    } else {
                        for (i, value) in [u32::MAX, u32::MAX, 0].into_iter().enumerate() {
                            self.write_glk_word(arg(1), i, value)?;
                        }
                    }
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

fn normalize_date(date: [i32; 8]) -> Option<NaiveDateTime> {
    let month = date[1] as i64 - 1;
    let year = i32::try_from(date[0] as i64 + month.div_euclid(12)).ok()?;
    NaiveDate::from_ymd_opt(year, month.rem_euclid(12) as u32 + 1, 1)?
        .and_hms_opt(0, 0, 0)?
        .checked_add_signed(Duration::days(date[2] as i64 - 1))?
        .checked_add_signed(Duration::hours(date[4] as i64))?
        .checked_add_signed(Duration::minutes(date[5] as i64))?
        .checked_add_signed(Duration::seconds(date[6] as i64))?
        .checked_add_signed(Duration::microseconds(date[7] as i64))
}

fn local_time<T: TimeZone>(zone: &T, naive: NaiveDateTime) -> Option<DateTime<T>> {
    if let Some(time) = zone.from_local_datetime(&naive).earliest() {
        return Some(time);
    }
    // A nonexistent wall time in a spring clock jump is normalized forward
    // using the offset before the gap, as mktime commonly does. Include date
    // line changes that skipped an entire day, not just one-hour DST shifts.
    for half_hours in 1..=96 {
        let earlier = naive.checked_sub_signed(Duration::minutes(half_hours * 30))?;
        if let Some(previous) = zone.from_local_datetime(&earlier).earliest() {
            let offset = previous.offset().fix().local_minus_utc();
            let utc = naive.checked_sub_signed(Duration::seconds(offset as i64))?;
            return Some(zone.from_utc_datetime(&utc));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn vm() -> Vm {
        Vm::new(
            Story::from_bytes(
                &super::super::tests::image_with_program(&[0x81, 0x20]),
                None,
            )
            .unwrap(),
        )
        .unwrap()
    }
    #[test]
    fn unsupported_calendar_dates_return_failure_without_stopping_execution() {
        let mut vm = vm();
        for date in [
            [i32::MAX, 1, 1, 0, 0, 0, 0, 0],
            [2000, i32::MIN, 1, 0, 0, 0, 0, 0],
            [2000, 1, i32::MAX, 0, 0, 0, 0, 0],
        ] {
            for (i, field) in date.into_iter().enumerate() {
                vm.memory
                    .write32(0x100 + i as u32 * 4, field as u32)
                    .unwrap();
            }
            for selector in [0x16c, 0x16d] {
                vm.datetime_call(selector, &[0x100, 0x140]).unwrap();
                assert_eq!(vm.memory.read32(0x140).unwrap(), u32::MAX);
                assert_eq!(vm.memory.read32(0x144).unwrap(), u32::MAX);
            }
            for selector in [0x16e, 0x16f] {
                assert_eq!(vm.datetime_call(selector, &[0x100, 60]).unwrap(), u32::MAX);
            }
            assert_eq!(vm.state(), RunState::Running);
        }
    }
    #[cfg(unix)]
    #[test]
    fn local_calendar_normalizes_dst_gaps_in_isolated_timezones() {
        let marker = "GLULX_DATETIME_TEST_ZONE";
        if let Ok(zone) = std::env::var(marker) {
            let (date, expected) = if zone == "America/New_York" {
                ([2024, 3, 10, 0, 2, 30, 0, 0], "2024-03-10T07:30:00+00:00")
            } else {
                ([2011, 12, 30, 0, 12, 0, 0, 0], "2011-12-30T22:00:00+00:00")
            };
            assert_eq!(
                local_time(&Local, normalize_date(date).unwrap())
                    .unwrap()
                    .with_timezone(&Utc)
                    .to_rfc3339(),
                expected
            );
            return;
        }
        for zone in ["America/New_York", "Pacific/Apia"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "vm::datetime::tests::local_calendar_normalizes_dst_gaps_in_isolated_timezones",
                ])
                .env("TZ", zone)
                .env(marker, zone)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{zone}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn calendar_normalization_handles_negative_and_overflowing_fields() {
        let date = normalize_date([2024, 14, 0, 999, 25, -1, 61, -1]).unwrap();
        assert_eq!(date.to_string(), "2025-02-01 01:00:00.999999");
        let fixed = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(local_time(&fixed, date).unwrap().naive_local(), date);
    }
}
