use solana_pubkey::Pubkey;
use spl_tollbooth_core::types::{TokenAmount, display_amount};

#[test]
fn parse_whole_number() {
    let mint = Pubkey::new_unique();
    let ta = TokenAmount::from_display("1", mint, 6).unwrap();
    assert_eq!(ta.raw, 1_000_000);
}

#[test]
fn parse_decimal() {
    let mint = Pubkey::new_unique();
    let ta = TokenAmount::from_display("0.001", mint, 6).unwrap();
    assert_eq!(ta.raw, 1_000);
}

#[test]
fn parse_exact_decimals() {
    let mint = Pubkey::new_unique();
    let ta = TokenAmount::from_display("1.123456", mint, 6).unwrap();
    assert_eq!(ta.raw, 1_123_456);
}

#[test]
fn reject_excess_precision() {
    let mint = Pubkey::new_unique();
    let result = TokenAmount::from_display("0.0000001", mint, 6);
    assert!(result.is_err());
}

#[test]
fn parse_zero_decimals() {
    let mint = Pubkey::new_unique();
    let ta = TokenAmount::from_display("42", mint, 0).unwrap();
    assert_eq!(ta.raw, 42);
}

#[test]
fn reject_zero_decimals_with_fraction() {
    let mint = Pubkey::new_unique();
    let result = TokenAmount::from_display("42.5", mint, 0);
    assert!(result.is_err());
}

#[test]
fn display_round_trip() {
    let mint = Pubkey::new_unique();
    let ta = TokenAmount::from_display("0.001", mint, 6).unwrap();
    assert_eq!(ta.display(), "0.001");
}

#[test]
fn reject_invalid_format() {
    let mint = Pubkey::new_unique();
    assert!(TokenAmount::from_display("abc", mint, 6).is_err());
    assert!(TokenAmount::from_display("1.2.3", mint, 6).is_err());
    assert!(TokenAmount::from_display("", mint, 6).is_err());
}

// --- display_amount() tests ---

#[test]
fn display_amount_fractional() {
    assert_eq!(display_amount(1000, 6), "0.001");
}

#[test]
fn display_amount_whole() {
    assert_eq!(display_amount(1_000_000, 6), "1");
}

#[test]
fn display_amount_trailing_zeros_trimmed() {
    assert_eq!(display_amount(1_500_000, 6), "1.5");
}

#[test]
fn display_amount_zero_decimals() {
    assert_eq!(display_amount(123, 0), "123");
}

#[test]
fn display_amount_zero_value() {
    assert_eq!(display_amount(0, 6), "0");
}
