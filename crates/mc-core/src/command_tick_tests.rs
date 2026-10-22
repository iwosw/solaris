use super::*;

#[test]
fn time_argument_scales_units_and_enforces_the_minimum() {
    assert_eq!(time_argument("5", 1).unwrap(), 5);
    assert_eq!(time_argument("5t", 1).unwrap(), 5);
    assert_eq!(time_argument("1.5s", 1).unwrap(), 30);
    assert_eq!(time_argument("1d", 1).unwrap(), 24_000);
    assert!(
        time_argument("0", 1)
            .unwrap_err()
            .message()
            .contains("Tick count must not be less than 1, found 0")
    );
    assert!(time_argument("5x", 1).is_err());
    assert!(time_argument("", 1).is_err());
    assert!(time_argument("abcs", 1).is_err());
}

#[test]
fn rate_accepts_the_vanilla_float_range_only() {
    assert_eq!(parse(&["rate", "1"]).unwrap(), TickCommand::Rate(1.0));
    assert!(parse(&["rate", "0.99"]).is_err());
    assert!(parse(&["rate", "nan"]).is_err());
    assert!(parse(&["rate"]).is_err());
    assert!(parse(&["rate", "20", "x"]).is_err());
}
