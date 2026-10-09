use super::*;

#[test]
fn round_trips_and_validates() {
    assert_eq!(parse("1970-01-01").unwrap(), 0);
    assert_eq!(format(parse("2026-11-10").unwrap()), "2026-11-10");
    assert_eq!(add("2026-12-31", 1).unwrap(), "2027-01-01");
    assert_eq!(add("2028-02-28", 1).unwrap(), "2028-02-29");
    assert_eq!(add("2026-03-01", -1).unwrap(), "2026-02-28");
    for bad in [
        "2026-02-29",
        "2026-13-01",
        "2026-1-01",
        "20261110",
        "2026-11-1x",
    ] {
        assert!(parse(bad).is_err(), "{bad} should be rejected");
    }
}

#[test]
fn weekdays() {
    assert_eq!(weekday("2026-11-10").unwrap(), "Tue");
    assert_eq!(weekday("1970-01-01").unwrap(), "Thu");
    assert_eq!(weekday("1969-12-31").unwrap(), "Wed");
}
