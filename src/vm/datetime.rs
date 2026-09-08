use super::*;
use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, Offset, TimeZone, Utc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Timestamp {
    seconds: i64,
    micros: u32,
}

impl Vm {
    pub(super) fn datetime_call(&mut self, selector: u32, args: &[u32]) -> Result<u32, VmError> {
        let arg = |n: usize| args.get(n).copied().unwrap_or(0);
        match selector {
            0x160 => {
                let now = Utc::now();
                self.write_time(
                    arg(0),
                    Timestamp {
                        seconds: now.timestamp(),
                        micros: now.timestamp_subsec_micros(),
                    },
                )?;
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
                    Timestamp {
                        seconds: arg(0) as i32 as i64 * arg(1) as i64,
                        micros: 0,
                    }
                } else {
                    let hi = self.read_glk_word(arg(0), 0)?;
                    let lo = self.read_glk_word(arg(0), 1)?;
                    let micros = self.read_glk_word(arg(0), 2)? as i32 as i64;
                    let seconds = ((hi as i64) << 32) | lo as i64;
                    Timestamp {
                        seconds: seconds
                            .checked_add(micros.div_euclid(1_000_000))
                            .ok_or(VmError::InvalidTime)?,
                        micros: micros.rem_euclid(1_000_000) as u32,
                    }
                };
                let destination = arg(if simple { 2 } else { 1 });
                let date = calendar_fields(if selector & 1 == 0 {
                    time
                } else {
                    local_wall_time(time).ok_or(VmError::InvalidTime)?
                });
                // A timestamp can exceed even the signed 32-bit Glk year.
                i32::try_from(date[0]).map_err(|_| VmError::InvalidTime)?;
                for (i, value) in date.into_iter().enumerate() {
                    self.write_glk_word(destination, i, value as u32)?;
                }
                Ok(0)
            }
            0x16c..=0x16f => {
                let mut date = [0i32; 8];
                for (i, value) in date.iter_mut().enumerate() {
                    *value = self.read_glk_word(arg(0), i)? as i32;
                }
                let wall = normalize_date(date);
                let time = if selector & 1 == 0 {
                    Some(wall)
                } else {
                    local_timestamp(wall)
                };
                if selector >= 0x16e {
                    if arg(1) == 0 {
                        return Err(VmError::InvalidTime);
                    }
                    Ok(time.map_or(u32::MAX, |v| v.seconds.div_euclid(arg(1) as i64) as u32))
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
    fn write_time(&mut self, address: u32, time: Timestamp) -> Result<(), VmError> {
        let seconds = time.seconds;
        for (i, value) in [(seconds >> 32) as u32, seconds as u32, time.micros]
            .into_iter()
            .enumerate()
        {
            self.write_glk_word(address, i, value)?;
        }
        Ok(())
    }
}
fn normalize_date(date: [i32; 8]) -> Timestamp {
    let month = date[1] as i64 - 1;
    let year = date[0] as i64 + month.div_euclid(12);
    let days = days_from_civil(year, month.rem_euclid(12) + 1, 1) + date[2] as i64 - 1;
    // Every possible combination of signed 32-bit fields fits in i64 seconds.
    Timestamp {
        seconds: days * 86400
            + date[4] as i64 * 3600
            + date[5] as i64 * 60
            + date[6] as i64
            + (date[7] as i64).div_euclid(1_000_000),
        micros: date[7].rem_euclid(1_000_000) as u32,
    }
}

// Gregorian dates repeat every 400 years (146097 days). Move January and
// February to the preceding year so the leap day ends each arithmetic year.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719468
}

fn calendar_fields(time: Timestamp) -> [i64; 8] {
    let days = time.seconds.div_euclid(86400);
    let shifted = days + 719468;
    let era = shifted.div_euclid(146097);
    let day_of_era = shifted.rem_euclid(146097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = march_month + if march_month < 10 { 3 } else { -9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    let seconds = time.seconds.rem_euclid(86400);
    [
        year,
        month,
        day,
        (days + 4).rem_euclid(7),
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        time.micros as i64,
    ]
}

fn native_datetime(time: Timestamp) -> Option<NaiveDateTime> {
    DateTime::from_timestamp(time.seconds, time.micros * 1000).map(|v| v.naive_utc())
}

fn timezone_date(time: Timestamp) -> NaiveDateTime {
    if let Some(date) = native_datetime(time) {
        return date;
    }
    let [year, month, day, _, hour, minute, second, micros] = calendar_fields(time);
    // Outside chrono's calendar range, only query the platform's timezone
    // offset. Future POSIX transition rules recur on the Gregorian 400-year
    // cycle. Ancient dates use an ancient representative, before the first
    // TZif transition, preserving the initial historical offset without DST.
    let year = if year < 0 { -2400 } else { 2400 } + year.rem_euclid(400) as i32;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32)
        .unwrap()
        .and_hms_micro_opt(hour as u32, minute as u32, second as u32, micros as u32)
        .unwrap()
}

fn local_wall_time(time: Timestamp) -> Option<Timestamp> {
    let naive = timezone_date(time);
    let offset = Local.offset_from_utc_datetime(&naive).local_minus_utc() as i64;
    Some(Timestamp {
        seconds: time.seconds.checked_add(offset)?,
        ..time
    })
}

fn local_timestamp(wall: Timestamp) -> Option<Timestamp> {
    let naive = timezone_date(wall);
    let local = local_time(&Local, naive)?;
    let adjustment = local.timestamp() - naive.and_utc().timestamp();
    Some(Timestamp {
        seconds: wall.seconds.checked_add(adjustment)?,
        ..wall
    })
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
    fn utc_calendar_covers_the_signed_glk_year_range() {
        let mut vm = vm();
        for (seconds, date) in [
            (
                10_000_000_000_000i64,
                [318857, 5, 20, 0, 17, 46, 40, 123456],
            ),
            (-10_000_000_000_000, [-314918, 8, 13, 0, 6, 13, 20, 123456]),
            (-62167219200, [0, 1, 1, 6, 0, 0, 0, 123456]),
            (-62167219201, [-1, 12, 31, 5, 23, 59, 59, 123456]),
            (67767976201996800, [i32::MAX, 1, 1, 2, 0, 0, 0, 123456]),
            (-67768100567971200, [i32::MIN, 1, 1, 2, 0, 0, 0, 123456]),
        ] {
            vm.write_time(
                0x100,
                Timestamp {
                    seconds,
                    micros: 123456,
                },
            )
            .unwrap();
            vm.datetime_call(0x168, &[0x100, 0x120]).unwrap();
            for (i, field) in date.iter().enumerate() {
                assert_eq!(
                    vm.memory.read32(0x120 + i as u32 * 4).unwrap() as i32,
                    *field,
                    "timestamp {seconds}, field {i}"
                );
            }
            vm.datetime_call(0x16c, &[0x120, 0x100]).unwrap();
            assert_eq!(vm.memory.read32(0x100).unwrap(), (seconds >> 32) as u32);
            assert_eq!(vm.memory.read32(0x104).unwrap(), seconds as u32);
            assert_eq!(vm.memory.read32(0x108).unwrap(), 123456);
            assert_eq!(
                vm.datetime_call(0x16e, &[0x120, 86400]).unwrap(),
                seconds.div_euclid(86400) as u32
            );
        }
    }

    #[test]
    fn simple_dates_and_fractional_seconds_normalize_outside_native_range() {
        let mut vm = vm();
        vm.datetime_call(0x16a, &[100_000_000, 86400, 0x100])
            .unwrap();
        assert_eq!(vm.memory.read32(0x100).unwrap(), 275760);
        assert_eq!(vm.memory.read32(0x104).unwrap(), 9);
        assert_eq!(vm.memory.read32(0x108).unwrap(), 13);
        vm.write_time(
            0x140,
            Timestamp {
                seconds: 10_000_000_000_000,
                micros: 0,
            },
        )
        .unwrap();
        vm.memory.write32(0x148, u32::MAX).unwrap();
        vm.datetime_call(0x168, &[0x140, 0x100]).unwrap();
        assert_eq!(vm.memory.read32(0x118).unwrap(), 39);
        assert_eq!(vm.memory.read32(0x11c).unwrap(), 999999);
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
                local_time(&Local, native_datetime(normalize_date(date)).unwrap())
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
        let date = native_datetime(normalize_date([2024, 14, 0, 999, 25, -1, 61, -1])).unwrap();
        assert_eq!(date.to_string(), "2025-02-01 01:00:00.999999");
        let fixed = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(local_time(&fixed, date).unwrap().naive_local(), date);
        for (fields, seconds, micros) in [
            ([2000, i32::MIN, 1, 0, 0, 0, 0, 0], -5647335589411200, 0),
            ([2000, 1, i32::MAX, 0, 0, 0, 0, 0], 185543533699200, 0),
            (
                [
                    i32::MAX,
                    i32::MAX,
                    i32::MAX,
                    0,
                    i32::MAX,
                    i32::MAX,
                    i32::MAX,
                    i32::MAX,
                ],
                73608717254619414,
                483647,
            ),
        ] {
            assert_eq!(normalize_date(fields), Timestamp { seconds, micros });
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_calendar_preserves_offsets_outside_native_year_range() {
        let marker = "GLULX_WIDE_DATETIME_TEST_ZONE";
        if let Ok(zone) = std::env::var(marker) {
            let offsets = match zone.as_str() {
                "UTC0" => [0, 0],
                "TST-8" => [28800, 28800],
                "America/New_York" => [-14400, -17762],
                "Pacific/Apia" => [46800, 45184],
                _ => unreachable!(),
            };
            for (seconds, offset) in [10_000_000_000_000, -10_000_000_000_000]
                .into_iter()
                .zip(offsets)
            {
                let stamp = Timestamp {
                    seconds,
                    micros: 123456,
                };
                let wall = local_wall_time(stamp).unwrap();
                assert_eq!(wall.seconds, seconds + offset, "{zone}: {seconds}");
                assert_eq!(local_timestamp(wall), Some(stamp));
            }
            return;
        }
        for zone in ["UTC0", "TST-8", "America/New_York", "Pacific/Apia"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "vm::datetime::tests::local_calendar_preserves_offsets_outside_native_year_range"])
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
}
