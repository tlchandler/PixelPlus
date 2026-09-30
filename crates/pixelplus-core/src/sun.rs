//! Sunrise and sunset times.
//!
//! Implements the NOAA solar calculator (after Jean Meeus, *Astronomical
//! Algorithms*), accurate to about a minute between ±72° latitude and still
//! well-behaved closer to the poles. Times are returned in UTC; convert with
//! [`chrono::DateTime::with_timezone`] for display.
//!
//! The sunrise/sunset definition is the standard one: the moment the upper limb
//! of the sun touches the horizon, including atmospheric refraction (a solar
//! zenith angle of 90.833°).
//!
//! ```
//! use chrono::NaiveDate;
//! use pixelplus_core::sun::sun_times;
//!
//! // Chicago on the winter solstice.
//! let times = sun_times(41.8781, -87.6298, NaiveDate::from_ymd_opt(2025, 12, 21).unwrap());
//! let sunset = times.sunset.unwrap();
//! assert_eq!(sunset.format("%H:%M").to_string(), "22:22"); // 4:22 pm CST
//! ```

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use serde::Serialize;

/// Zenith angle (degrees) of the sun's centre at official sunrise/sunset.
const OFFICIAL_ZENITH_DEG: f64 = 90.833;

/// What kind of day a location experiences on a given date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DayKind {
    /// The sun rises and sets.
    Normal,
    /// Midnight sun: the sun stays above the horizon all day.
    PolarDay,
    /// Polar night: the sun stays below the horizon all day.
    PolarNight,
}

/// Solar events for one calendar date at one location.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SunTimes {
    /// Sunrise, or `None` when the sun does not rise or set that day.
    pub sunrise: Option<DateTime<Utc>>,
    /// Sunset, or `None` when the sun does not rise or set that day.
    pub sunset: Option<DateTime<Utc>>,
    /// Moment the sun is highest in the sky (always defined).
    pub solar_noon: DateTime<Utc>,
    /// Whether this is a normal day, polar day or polar night.
    pub day_kind: DayKind,
}

impl SunTimes {
    /// Sunset if the sun sets today; otherwise the closest meaningful
    /// substitute so that sunset-relative schedules keep working near the
    /// poles: solar midnight during polar day (when the sun is lowest), solar
    /// noon during polar night (it is already dark).
    pub fn sunset_or_fallback(&self) -> DateTime<Utc> {
        match (self.sunset, self.day_kind) {
            (Some(t), _) => t,
            (None, DayKind::PolarDay) => self.solar_noon + Duration::hours(12),
            (None, _) => self.solar_noon,
        }
    }

    /// Sunrise if the sun rises today; otherwise solar midnight before
    /// solar noon during polar day and solar noon during polar night.
    pub fn sunrise_or_fallback(&self) -> DateTime<Utc> {
        match (self.sunrise, self.day_kind) {
            (Some(t), _) => t,
            (None, DayKind::PolarDay) => self.solar_noon - Duration::hours(12),
            (None, _) => self.solar_noon,
        }
    }
}

/// Compute sunrise, sunset and solar noon for `date` at `lat`/`lon`
/// (degrees; north and east positive).
///
/// `date` is the *local* calendar date of the observer. Because the result is
/// in UTC, an event may fall on the previous or next UTC day (e.g. sunset in
/// Chicago is after midnight UTC in summer).
///
/// Out-of-range input never panics: latitude is clamped to ±90°, longitude is
/// wrapped into ±180°, and non-finite values are treated as 0.
pub fn sun_times(lat: f64, lon: f64, date: NaiveDate) -> SunTimes {
    let lat = if lat.is_finite() {
        lat.clamp(-90.0, 90.0)
    } else {
        0.0
    };
    let lon = if lon.is_finite() {
        (lon + 180.0).rem_euclid(360.0) - 180.0
    } else {
        0.0
    };
    let midnight = date.and_time(NaiveTime::MIN).and_utc();
    let jd0 = julian_day(date);

    // Solar noon, refined once using the solar position at the estimate.
    let mut noon_min = 720.0 - 4.0 * lon;
    for _ in 0..2 {
        let pos = SolarPosition::at(jd0 + noon_min / 1440.0);
        noon_min = 720.0 - 4.0 * lon - pos.equation_of_time_min;
    }
    let noon_pos = SolarPosition::at(jd0 + noon_min / 1440.0);
    let day_kind = match hour_angle_deg(lat, noon_pos.declination_deg) {
        HourAngle::Angle(_) => DayKind::Normal,
        HourAngle::AlwaysUp => DayKind::PolarDay,
        HourAngle::AlwaysDown => DayKind::PolarNight,
    };

    let event = |rising: bool| -> Option<DateTime<Utc>> {
        if day_kind != DayKind::Normal {
            return None;
        }
        // Start from the noon position, then iterate using the solar position
        // at the event time itself for accuracy.
        let mut minutes = noon_min;
        for _ in 0..3 {
            let pos = SolarPosition::at(jd0 + minutes / 1440.0);
            let ha = match hour_angle_deg(lat, pos.declination_deg) {
                HourAngle::Angle(h) => h,
                // Only reachable right at the polar-circle transition; the
                // noon-based classification wins, so keep the last estimate.
                _ => break,
            };
            let signed = if rising { ha } else { -ha };
            minutes = 720.0 - 4.0 * (lon + signed) - pos.equation_of_time_min;
        }
        Some(midnight + minutes_to_duration(minutes))
    };

    SunTimes {
        sunrise: event(true),
        sunset: event(false),
        solar_noon: midnight + minutes_to_duration(noon_min),
        day_kind,
    }
}

/// Sunset (UTC) for `date`, or `None` during polar day/night.
pub fn sunset(lat: f64, lon: f64, date: NaiveDate) -> Option<DateTime<Utc>> {
    sun_times(lat, lon, date).sunset
}

/// Sunrise (UTC) for `date`, or `None` during polar day/night.
pub fn sunrise(lat: f64, lon: f64, date: NaiveDate) -> Option<DateTime<Utc>> {
    sun_times(lat, lon, date).sunrise
}

fn minutes_to_duration(minutes: f64) -> Duration {
    // Round to whole seconds; the algorithm is not more accurate than that.
    Duration::seconds((minutes * 60.0).round() as i64)
}

/// Julian day number at 00:00 UTC of `date`.
fn julian_day(date: NaiveDate) -> f64 {
    // 2000-01-01 00:00 UTC is JD 2451544.5.
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid constant date");
    2_451_544.5 + (date - epoch).num_days() as f64
}

enum HourAngle {
    Angle(f64),
    AlwaysUp,
    AlwaysDown,
}

fn hour_angle_deg(lat_deg: f64, decl_deg: f64) -> HourAngle {
    let lat = lat_deg.to_radians();
    let decl = decl_deg.to_radians();
    let cos_h =
        OFFICIAL_ZENITH_DEG.to_radians().cos() / (lat.cos() * decl.cos()) - lat.tan() * decl.tan();
    if !cos_h.is_finite() {
        // Exactly at a pole: the sun is up iff it is in that hemisphere.
        return if (lat_deg >= 0.0) == (decl_deg > 0.0) {
            HourAngle::AlwaysUp
        } else {
            HourAngle::AlwaysDown
        };
    }
    if cos_h > 1.0 {
        HourAngle::AlwaysDown
    } else if cos_h < -1.0 {
        HourAngle::AlwaysUp
    } else {
        HourAngle::Angle(cos_h.acos().to_degrees())
    }
}

struct SolarPosition {
    declination_deg: f64,
    equation_of_time_min: f64,
}

impl SolarPosition {
    fn at(jd: f64) -> Self {
        let t = (jd - 2_451_545.0) / 36_525.0;
        let l0 = (280.46646 + t * (36_000.769_83 + t * 0.0003032)).rem_euclid(360.0);
        let m = 357.52911 + t * (35_999.050_29 - 0.0001537 * t);
        let e = 0.016708634 - t * (0.000042037 + 0.0000001267 * t);
        let m_rad = m.to_radians();
        let c = m_rad.sin() * (1.914602 - t * (0.004817 + 0.000014 * t))
            + (2.0 * m_rad).sin() * (0.019993 - 0.000101 * t)
            + (3.0 * m_rad).sin() * 0.000289;
        let true_long = l0 + c;
        let omega = (125.04 - 1934.136 * t).to_radians();
        let apparent_long = true_long - 0.00569 - 0.00478 * omega.sin();
        let mean_obliquity =
            23.0 + (26.0 + (21.448 - t * (46.815 + t * (0.00059 - t * 0.001813))) / 60.0) / 60.0;
        let obliquity = (mean_obliquity + 0.00256 * omega.cos()).to_radians();
        let declination = (obliquity.sin() * apparent_long.to_radians().sin()).asin();

        let y = (obliquity / 2.0).tan().powi(2);
        let l0r = l0.to_radians();
        let eq_time = y * (2.0 * l0r).sin() - 2.0 * e * m_rad.sin()
            + 4.0 * e * y * m_rad.sin() * (2.0 * l0r).cos()
            - 0.5 * y * y * (4.0 * l0r).sin()
            - 1.25 * e * e * (2.0 * m_rad).sin();

        SolarPosition {
            declination_deg: declination.to_degrees(),
            equation_of_time_min: 4.0 * eq_time.to_degrees(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn assert_near(actual: DateTime<Utc>, expected: DateTime<Utc>, tolerance_min: i64) {
        let diff = (actual - expected).num_seconds().abs();
        assert!(
            diff <= tolerance_min * 60,
            "expected {expected}, got {actual} (off by {diff}s)"
        );
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn chicago_winter_solstice() {
        // NOAA: sunrise 07:15 CST, sunset 16:22 CST.
        let t = sun_times(41.8781, -87.6298, date(2025, 12, 21));
        assert_eq!(t.day_kind, DayKind::Normal);
        assert_near(t.sunrise.unwrap(), utc(2025, 12, 21, 13, 15), 2);
        assert_near(t.sunset.unwrap(), utc(2025, 12, 21, 22, 22), 2);
        assert_near(t.solar_noon, utc(2025, 12, 21, 17, 48), 2);
    }

    #[test]
    fn london_summer_solstice() {
        // NOAA: sunrise 04:43 BST, sunset 21:21 BST.
        let t = sun_times(51.5074, -0.1278, date(2026, 6, 21));
        assert_near(t.sunrise.unwrap(), utc(2026, 6, 21, 3, 43), 2);
        assert_near(t.sunset.unwrap(), utc(2026, 6, 21, 20, 21), 2);
    }

    #[test]
    fn sydney_crosses_utc_day() {
        // NOAA: sunrise 05:41 AEDT (18:41 UTC previous day), sunset 20:05 AEDT.
        let t = sun_times(-33.8688, 151.2093, date(2025, 12, 21));
        assert_near(t.sunrise.unwrap(), utc(2025, 12, 20, 18, 41), 2);
        assert_near(t.sunset.unwrap(), utc(2025, 12, 21, 9, 5), 2);
    }

    #[test]
    fn polar_night_and_day() {
        let tromso = (69.6492, 18.9553);
        let winter = sun_times(tromso.0, tromso.1, date(2025, 12, 21));
        assert_eq!(winter.day_kind, DayKind::PolarNight);
        assert!(winter.sunrise.is_none() && winter.sunset.is_none());
        assert_eq!(winter.sunset_or_fallback(), winter.solar_noon);

        let summer = sun_times(tromso.0, tromso.1, date(2026, 6, 21));
        assert_eq!(summer.day_kind, DayKind::PolarDay);
        assert_eq!(
            summer.sunset_or_fallback(),
            summer.solar_noon + Duration::hours(12)
        );
        assert_eq!(
            summer.sunrise_or_fallback(),
            summer.solar_noon - Duration::hours(12)
        );
    }

    #[test]
    fn southern_polar_seasons_are_reversed() {
        let mcmurdo = (-77.8419, 166.6863);
        assert_eq!(
            sun_times(mcmurdo.0, mcmurdo.1, date(2025, 12, 21)).day_kind,
            DayKind::PolarDay
        );
        assert_eq!(
            sun_times(mcmurdo.0, mcmurdo.1, date(2026, 6, 21)).day_kind,
            DayKind::PolarNight
        );
    }

    #[test]
    fn bad_input_does_not_panic() {
        let d = date(2026, 3, 20);
        for (lat, lon) in [
            (f64::NAN, f64::NAN),
            (f64::INFINITY, 0.0),
            (90.0, 0.0),
            (-90.0, 0.0),
            (200.0, 1000.0),
        ] {
            let t = sun_times(lat, lon, d);
            if let (Some(r), Some(s)) = (t.sunrise, t.sunset) {
                assert!(r < s);
            }
        }
        // Longitude wraps: 360° east is the same as 0°.
        assert_eq!(sun_times(10.0, 360.0, d), sun_times(10.0, 0.0, d));
    }

    #[test]
    fn equator_equinox_is_about_twelve_hours() {
        let t = sun_times(0.0, 0.0, date(2026, 3, 20));
        let len = t.sunset.unwrap() - t.sunrise.unwrap();
        // Refraction makes the day a few minutes longer than 12 h.
        assert!((len.num_minutes() - 727).abs() <= 3, "{len}");
        assert!(sunrise(0.0, 0.0, date(2026, 3, 20)).is_some());
        assert!(sunset(0.0, 0.0, date(2026, 3, 20)).is_some());
    }
}
