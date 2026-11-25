use chrono::{Datelike, Local, Timelike, Utc};
use std::time::{SystemTime, UNIX_EPOCH};

/// Local calendar ticks since year1, in100ns units. Kind/ambiguous-DST flags do
/// not participate in the original DateTime subtraction used by history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryTimestamp(i64);
impl HistoryTimestamp {
    pub fn ticks(self) -> i64 {
        self.0
    }

    pub(super) fn seconds_since(self, earlier: Self) -> f64 {
        // mscorlib0600064f subtracts local ticks;0600134d converts then multiplies.
        (self.0 - earlier.0) as f64 * 1e-7
    }
}

pub(super) enum Clock {
    Local,
    #[cfg(test)]
    Script(std::sync::Mutex<std::collections::VecDeque<Result<i64, &'static str>>>),
}
impl Clock {
    pub(super) fn now(&self) -> Result<HistoryTimestamp, &'static str> {
        match self {
            Self::Local => {
                // Chrono Local::now uses Utc::now, which panics before1970.
                // Convert the actual SystemTime through its fallible calendar API
                // so ordinary pre-epoch wall clocks keep their signed meaning.
                calendar_time(SystemTime::now())
            }
            #[cfg(test)]
            Self::Script(values) => values
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pop_front()
                .expect("finite source-law clock exhausted")
                .map(HistoryTimestamp),
        }
    }
}

fn calendar_time(now: SystemTime) -> Result<HistoryTimestamp, &'static str> {
    let (seconds, nanoseconds) = match now.duration_since(UNIX_EPOCH) {
        Ok(duration) => (i128::from(duration.as_secs()), duration.subsec_nanos()),
        Err(error) => {
            let duration = error.duration();
            let seconds = -i128::from(duration.as_secs());
            if duration.subsec_nanos() == 0 {
                (seconds, 0)
            } else {
                (seconds - 1, 1_000_000_000 - duration.subsec_nanos())
            }
        }
    };
    let seconds =
        i64::try_from(seconds).map_err(|_| "system clock exceeds cached calendar capability")?;
    let utc = chrono::DateTime::<Utc>::from_timestamp(seconds, nanoseconds)
        .ok_or("system clock exceeds cached calendar capability")?;
    let local = utc.with_timezone(&Local).naive_local();
    Ok(local_ticks(local))
}

fn local_ticks(local: chrono::NaiveDateTime) -> HistoryTimestamp {
    let days = i128::from(local.date().num_days_from_ce()) - 1;
    let seconds = i128::from(local.time().num_seconds_from_midnight());
    let ticks =
        days * 864_000_000_000 + seconds * 10_000_000 + i128::from(local.time().nanosecond() / 100);
    // DateTime.Now saturates after applying the local offset. Chrono supplies
    // actual local wall time; its zone fallback/cache and leap-second profile
    // remain documented environmental qualifications, not exact old goldens.
    HistoryTimestamp(ticks.clamp(0, 3_155_378_975_999_999_999) as i64)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_calendar_ticks_preserve_year_one_epoch_truncation_and_source_clamp() {
        let date = |year| chrono::NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
        assert_eq!(
            local_ticks(date(1).and_hms_nano_opt(0, 0, 0, 199).unwrap()).ticks(),
            1
        );
        assert_eq!(
            local_ticks(date(0).and_hms_opt(23, 59, 59).unwrap()).ticks(),
            0
        );
        assert_eq!(
            local_ticks(date(10_000).and_hms_opt(0, 0, 0).unwrap()).ticks(),
            3_155_378_975_999_999_999
        );
    }
}
